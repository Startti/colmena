# 54. Tabular prepare (large CSV/Excel preparation)

Module `tabular_prepare` will hold the pieces that prepare a large tabular
attachment once, ahead of the questions asked about it. Everything in it is
**dark**: it is used only when the engine switch `COLMENA_LARGE_TABULAR=on`,
and nothing calls into the module yet (the CSV converter exists, see below,
but no host wires it). With the switch off
(the default) behaviour is unchanged and the registry table stays empty.

"Large" means strictly greater than 50 MiB (52,428,800 bytes); exactly 50 MiB
is small and never enters this module.

## Registry table

`attachment_prepared` (see [30_database_schema.md](./30_database_schema.md))
has one row per source `storage_key`. Statuses are `running`, `ready`,
`failed` and `deleting` (plain TEXT). A row is created when a worker claims the
preparation, so it starts at `running`; there is no `pending` row. An item that
is queued but not started is reported as pending from the queue and the
progress port while no row exists. `deleting` is held by the cleanup pass and is
never claimable by a preparation.

The migrations exist for SQLite and Postgres
(`migrations/{sqlite,postgres}/20261004000001_attachment_prepared.sql`).

## Registry

The table sits behind the `PreparationRegistry` trait (`registry.rs`) so the
host can decide where rows live and who deletes one (open question O8: ADP
directly or an internal Colmena endpoint). Both dialects run the same
statements: written once with `$N` placeholders, rewritten to `?N` for SQLite.
Two implementations exist: SQLite (`sqlite_registry.rs`) and Postgres
(`postgres_registry.rs`), with `claim`, `complete`, `fail`, the cancellation
check and `get`. The Postgres one is `from_pool(Arc<PgPool>)`; it runs the same
SQL text, with `TIMESTAMPTZ` compared natively.

### Claim rule

`claim` is one statement (`INSERT ... ON CONFLICT DO UPDATE ... WHERE ...
RETURNING`), atomic on both dialects. It is won when:

| Existing row | Claim |
|--------------|-------|
| none | yes, `attempts = 1` |
| `failed` with `attempts < 3` | yes, `attempts + 1` |
| `failed` with `attempts = 3` | no, the failure is final |
| `running` with a live lease (`lease_until >= now`) | no |
| `running` with an expired lease and `attempts < 3` | yes, `attempts + 1` |
| `running` with an expired lease and `attempts = 3` | no: abandoned (see below) |
| `ready` at the current `format_version` | no |
| `deleting` with a live lease | no |
| `deleting` with an expired lease | yes, a new life: `attempts = 1` |
| any state except `deleting` with an older `format_version` | yes, `attempts` restarts at 1 |

An expired-lease takeover counts as an attempt, so the 3-attempt bound holds
for jobs that die without writing `failed`. A row `running` with an expired
lease at 3 attempts is **abandoned**: the claim refuses it and no write is
needed to reach that state (the reader reports it as a final failure; that
comes with `ensure_prepared` in a later slice). The row stays until the TTL
pass removes it.

`now` is passed in by the caller, never read from the database clock, so both
dialects compare against the same value and tests control time. SQLite compares
the RFC 3339 text of whole-second and fractional timestamps; tests cover the
lease boundary at millisecond resolution. Two concurrent claims give exactly one
winner.

The lease is a single fixed value, `P_PREP_TIME + 60 s` (`lease_for`), with no
renewal: the registry is written on state change, not for progress.

### Terminal writes

The registry is written on state change: one claim and one terminal write per
attempt. `complete` and `fail`/`fail_with_blobs` are conditional on
`lease_owner = me` (and `status = 'running'`). Zero rows updated returns
`Cancelled`: the row was deleted or the lease was taken over, and the caller
must stop. A cancelled preparation never becomes `ready`, and a missing row
never gains a `failed` row. On success the lease is cleared; `failed` keeps its
attempt count and records the reason.

### Cleanup queries

Two read queries serve the cleanup pass (which arrives in later slices); both
are in `PreparationRegistry` and run on SQLite and Postgres:

- `find_stale(cutoff, now, after, limit)`: rows whose
  `COALESCE(last_used_at, created_at)` is before `cutoff`, in key order strictly
  after the `after` cursor, `limit` at a time. A `running` row with a live lease
  at `now` is never returned: an old row claimed again keeps its creation time
  and its preparation must not be pulled from under it. The keyset cursor means
  rows the pass cannot delete never hide the ones behind them. A table used
  recently is not returned, because `touch_last_used` moves `last_used_at`.
- `list_ready_after(after, limit)`: `ready` rows in key order after a cursor, for
  the pass that checks whether a manifest still exists.

### State machine

| From | To | By |
|------|----|----|
| (none) | `running` | `claim` |
| `failed` (attempts < 3), `running` with an expired lease (attempts < 3), any row of an older format except `deleting` | `running` | `claim` |
| `running` | `ready` / `failed` | `complete` / `fail` by the lease owner |
| `ready` | `failed(manifest_missing)` (attempts already 0) | cleanup, when the manifest is gone (`mark_manifest_missing`) |
| `ready`, `failed`, `running` with an expired lease | `deleting` | cleanup `begin_delete`, under its own lease, only if the row is unchanged since it was read (status, `updated_at`, `last_used_at`) |
| `deleting` | (row deleted) | cleanup `finish_delete`, after the derived blobs are gone |
| `deleting` with an expired lease | `running` | `claim`: the source is not stuck behind a cleanup that never finishes |
| `running` with a live lease | (row deleted) | the source file was deleted: the job's `still_owned` turns false and it stops |

The claim never takes a `deleting` row because of its format: only an expired
`deleting` lease makes it claimable.

**TTL clock.** Every claim sets `created_at` to the claim time and clears
`last_used_at`: a table prepared again (after a failed retry, a format change or
a lost manifest) starts its TTL clock at that preparation, so it cannot look
stale before its first use because its row was created long ago.

`mark_manifest_missing(observed_row, now)` turns a `ready` row whose manifest no
longer exists into `failed(manifest_missing)`, so the next claim prepares it
again. It applies only if the row is still exactly what the caller observed (same
`updated_at` and `manifest_key`): the check that found the manifest missing is a
snapshot, and a table prepared again since is not demoted by it. `complete`
clears `attempts`, so the cap bounds CONSECUTIVE failures: the demoted row starts
from a clean count, and a table that needed retries (or that loses its manifest
more than once) is not a permanent failure for that reason alone. A claim over an
older format keeps the old `manifest_key` in place; `complete`/`fail` add it to
the tracked `blob_keys` so cleanup can still reach it.

### Cleanup claim

A row is never deleted from under a live preparation. The cleanup pass first
CLAIMS the row with `begin_delete(row, owner, lease, now)`, which moves it to
`deleting` under the pass's own lease, only if the row is still exactly what the
pass read (same `status`, `updated_at` and `last_used_at`; the last uses a
portable null-safe equality) and holds no live lease. `false` means someone
changed it meanwhile (a preparation claimed it, or a table was just handed out):
leave it alone. After the derived blobs are gone, `finish_delete(key, owner)`
deletes the row, only while that owner still holds the `deleting` lease.

`delete_if_unchanged(row)` deletes a row only if it is still exactly what the
caller observed (same `status`, `updated_at` and lease owner). The cancellation
of a running preparation uses it (see Cleanup): a job that completed or was taken
over after the read changes the row, so the finished table is never lost from a
stale snapshot; the caller re-reads and follows the normal path instead.

`find_stale` also returns a `deleting` row whatever its age once its lease has
expired (a pass died mid-delete), so the next run takes it over; a `deleting` row
with a live lease is never returned.

### Cancellation check

- `still_owned(source_key, owner)` is the cancellation check a job makes
  between parts: a read by primary key, `false` for a missing row or another
  owner. It never writes, and it does not renew the lease (the lease is fixed).
- `delete(source_key)` removes the row unconditionally. It is used only when
  the SOURCE file is deleted: a running job then sees the missing row through
  `still_owned`, stops, and its `complete` returns `Cancelled`, so no table is
  ever completed for a source that no longer exists.
- `release(source_key, owner)` removes the row only if `owner` still holds it.
  A duplicate trigger that arrives after a delete re-claims a fresh row, finds
  the source missing and releases its own row, so no `failed` row is left
  behind.

### Blob tracking

`blob_keys` is every blob that may exist for the source. `complete` and
`fail_with_blobs` both write it as the **union** of what the row already tracks
and what the attempt reports (existing order first, no duplicates), so blobs of
an earlier failed attempt stay known; relative paths are deterministic, so a
later attempt overwrites instead of adding. The lease owner is the only writer,
so reading the tracked keys and then updating is safe. A claim keeps
`blob_keys` and clears the manifest, tables and error of the previous attempt.
A cancelled job deletes what it wrote. Blobs of an attempt that crashed before
any terminal write are not tracked; removing them needs a host that can delete
by prefix (a later slice adds that port) and is otherwise a known limit.

## Switch and ports

`COLMENA_LARGE_TABULAR=on` (also `true`, `1`, `yes`) is read once by
`EngineConfig::from_env` into `EngineConfig.prepare.large_tabular`; unset, `off`
or anything else means off, and a later change of the environment does not reach
a config already built. `EngineConfig.prepare` also carries the two ports a host
replaces:

- `PrepareTrigger::request(PrepareRequest)` asks the host to start a
  preparation. It is best-effort and idempotent: the job claims the registry
  row, so a duplicate request yields one preparation. The default
  `InlineTrigger` hands the request to a `PrepareRunner` in this process; with
  no converter wired (the CSV converter below is not wired to it yet) it only
  logs.
- `PrepareProgress::{report, read}` carries progress outside the registry
  (the ADP adapter keeps it in Redis). The default `NoopProgress` remembers
  nothing. Colmena itself has no Redis dependency.

**Start-up validation.** Enabling `COLMENA_LARGE_TABULAR` with the default,
unwired trigger is a configuration error: `ColmenaEngine::new` calls
`PrepareConfig::validate` and returns `EngineError::Other` naming the variable,
instead of letting every call wait and report "still preparing" forever. A host
that turns the switch on must first set `EngineConfig.prepare.trigger` to a
wired trigger (and `prepare.progress` if it wants progress) before building the
engine. With the switch off nothing is checked and nothing changes.

## `ensure_prepared`

`TabularPrepare::ensure_prepared(request, wait)` makes sure a source is
prepared, waiting at most `wait` (polled every second). The bound covers the
registry, trigger and progress calls too, not only the sleeps; each call is
always given a 2 s floor, so a zero or very short wait still reads the registry
and still reports a table that is already ready. The wait itself is capped (ten
years) so `Instant + wait` cannot overflow:

| Situation | Behaviour |
|-----------|-----------|
| Switch off | `NotEnabled`; the registry, the trigger and the progress port are not touched |
| `ready` | `Ready(row)` |
| `running` with a live lease | attach and wait; never a second trigger |
| No row | request once, unless the progress port says the host already queued or started it (then attach) |
| Retryable `failed` row, expired lease with attempts left, or older format | request once, then wait; the progress hint is NOT consulted (a stale one must not block the takeover) |
| `deleting` with a live lease | attach and wait; never a second trigger (checked before the format) |
| `deleting` with an expired lease | request once, then wait (the claim takes it) |
| Wait ends first | `StillPreparing { progress }`, never an error |
| `failed` after a request, attempts left | `Failed { final_failure: false }`: a later request tries again |
| `failed` with 3 attempts | `Failed { final_failure: true }` with the reason; never requested again |
| `running`, lease expired, 3 attempts | `Failed { final_failure: true }` with code `lease_expired` (abandoned); never requested |

A `failed` row found before the request is not mistaken for a new failure: only a
higher attempt count, or a failure when we started from no failure, ends the wait
early.

`EnsureOutcome` has no variant that offers the whole file: a file that is not
prepared is never loaded whole into the sandbox, and `EnsureOutcome::message()`
words the state for the model. The call never writes the registry: claiming is
the job's business. The attempt bound mirrors the registry's claim rule (3 attempts; an
expired-lease takeover counts as one).
**De-duplication.** Concurrent callers in one process share a request through a
set whose guard is created when the key is inserted, so every exit, including a
dropped future, removes it. It only saves duplicate triggers: callers in other
processes, or a later call after a wait ended, may request again, which is safe
because triggers are idempotent. A trigger error is returned as an error and is
not remembered as "already requested".

**Use is recorded.** When a ready table is handed out, `ensure_prepared` calls
`touch_last_used`, a conditional update that writes `last_used_at` only when it
is NULL or older than a day and only for a `ready` row. That is a write on use
(about one per row per day), not a preparation write and not progress, so the
"claim and terminal only" rule for the preparation itself still holds (a test
runs a whole job through `ensure_prepared` and counts exactly one claim, one
terminal write and one touch). It is what lets the TTL measure use instead of
creation. The call is bounded by its own short deadline and a failure to record
is logged and never fails or delays the answer. The decision is made from the row already read: when the use was recorded within the day (the normal case) the table is handed out with no write attempt and no extra read. Only when the touch was due and changed nothing (the cleanup pass took the row between our read and the touch) does `ensure_prepared` read the row again and hand the table out only if it is still `ready`; a row that is `deleting` is waited on and a missing row is requested, like any other source. A failed re-read is best-effort, like a failed touch: it is logged and the table already held is handed out (failing there would turn a bookkeeping hiccup into a tool error).

**Known limit.** With the switch on and a trigger that is wired but never starts
a job, every call ends `StillPreparing`; the unwired default cannot reach this
state because the engine refuses to start (see Switch and ports).

## Streamed storage

`OutputStorageRepository::store_stream(StoreStreamRequest)` persists a payload
that arrives as a stream. `placement` is `Generated` (today's layout) or
`DerivedFrom { source_storage_key, relative_path }` (a blob that lives and dies
with a source file). The default implementation buffers the stream in memory
and calls `store`, ignoring `placement`; it refuses with `InvalidInput("this
host does not support streamed storage ...")` a stream whose `size_hint` or
accumulated bytes exceed `DEFAULT_STREAM_BUFFER_MAX` (64 MiB, one prepared
part). **A host that stores larger outputs must override it** to upload in
chunks and honour `placement`. A stream error aborts before anything is
stored. `store` is unchanged.

Two more methods have safe defaults: `derived_root(source_key)` (the prefix
under which a source's derived blobs live; default `None` = unknown) and
`delete_derived(source_key, tracked_keys)` (default: delete each tracked key in
order, stopping at the first failure; a host that can delete by prefix overrides
it and thereby also removes blobs nobody tracked, such as those of an attempt
that crashed before any terminal write). Nothing calls them yet: the cleanup
pass that uses them follows in later slices.

## Cleanup

`gc.rs` holds the passes `attachment_gc` will run (the binary is wired in a later
slice). The first: when a source blob is deleted,
`delete_prepared_for_source(registry, storage, source_key, clock, dry_run)` removes
its prepared tables.

The pass claims the row (`begin_delete`, see Cleanup claim), calls
`OutputStorageRepository::delete_derived` with the tracked keys (manifest last)
and, last, `finish_delete`. If a preparation claimed the row after the pass read
it, `begin_delete` returns false and the row and its blobs are left alone. A dry
run only logs. With no row for the source the pass makes no storage call. The
passes read a `Clock` once per row, never once per pass, so each row's cleanup
lease (10 minutes) starts when the pass claims it.

**Failures and leases.** If storage refuses, the row stays `deleting`: while the
cleanup lease is live nothing else touches it, and once it expires the next run
(or a preparation, through the claim) takes it over, so a persistent failure
never leaves a source unpreparable; the failure is reported in the summary
(`storage_errors`) and in an error log. If a lease nevertheless expires while the
blobs are being deleted and a preparation claims the row, `finish_delete` returns
false: the pass logs it, counts it in `leases_lost` and leaves the new owner's row
alone.

**Incomplete is visible.** `PreparedGcSummary::is_incomplete()` is true when a blob
could not be deleted (`storage_errors`) or when the pass could not settle the source
(`busy`): the row had changed or is leased by another pass or a preparation.
Nothing was wrongly deleted, and the caller must retry on its next run.

**Containment.** Every deletion path validates the tracked keys with one
function before it deletes anything: never the source key itself, never a key with
a `..` segment, and only keys inside the root the host's storage adapter reports
with `derived_root`, compared on a path-segment boundary (`u/s/prepared-other/x`
is not inside `u/s/prepared`). **A host whose adapter reports no root (the default)
cannot have its keys contained, so the pass refuses to delete them**: the row is
left untouched, an error is logged and `keys_rejected` is counted. A row that
tracks no key has nothing to contain and is processed normally. A dry run
validates first, so it never promises a deletion the real run would refuse. In the
shipped binary no adapter overrides `derived_root` yet, so prepared tables are not
deleted until the host's adapter does (nothing writes them until the preparation
job exists); the counters show it.

## Tests

`cargo test --lib attachment_prepared` applies the SQLite migration to an
in-memory database and checks the columns, the primary key and the defaults.
The Postgres twin is `#[ignore]`d like the other Postgres repository tests and
needs `DATABASE_URL` (`DATABASE_URL=postgres://... cargo test --lib
attachment_prepared -- --ignored`).

`cargo test --lib attachment_gc` covers the cleanup passes against a real SQLite
registry and an in-memory fake storage. `cargo test --lib output_storage` covers the storage defaults (buffering, the
size cap, a stream error storing nothing, derived-blob deletion).
`cargo test --lib tabular_prepare_ensure` covers `ensure_prepared` with fake
ports and a fake registry under paused time, including a dropped future, a hung
progress port and a zero wait. `cargo test --test large_tabular_switch` runs in
its own process: the switch is off by default, a built config keeps its value
when the environment changes (shown through the outcome a caller sees), and the
engine refuses to start with the switch on and an unwired trigger.

`cargo test --lib tabular_prepare` runs the registry cases against a
file-backed SQLite database: a claim creates a `running` row, a live lease is
refused, an expired one is taken over, a dead job is retried only up to the
attempt cap, the lease boundary holds at millisecond resolution, and eight
concurrent claims have exactly one winner. Terminal writes, the blob union, the
`Cancelled` cases and the cancellation check (including that checking ownership
changes nothing, lease included) are covered the same way. The cases are written once as
functions generic over the trait (`registry_contract.rs`) so the Postgres implementation
runs the same ones.

The same cases run against Postgres from `postgres_registry.rs` as
`tabular_prepare_pg_*`. They are `#[ignore]`d like the other Postgres repository
tests and need `DATABASE_URL`:
`DATABASE_URL=postgres://... cargo test --lib tabular_prepare -- --ignored`.

## Conversion (CSV to Parquet)

A large CSV is converted into typed Parquet parts by a trusted job, never by
code the model writes. The converter is built from small modules, each dark
(nothing calls them while `COLMENA_LARGE_TABULAR` is off). The sections below
follow the order the data takes: the manifest and the part sink and writer that
produce the output, then reading and typing a CSV, then the conversion that ties
them together.

### Manifest (`manifest.rs`)

`manifest.json` lists, per table, its name, row count, part count and, per
column, the name, type (`int`, `float`, `bool`, `string`, `date`, `timestamp`)
and two sizes: `uncompressed_bytes` is the Parquet size as encoded (a
dictionary-encoded string column is far smaller than its values; for information
and for sizing storage) and `in_memory_bytes` is the column's decoded size in
Arrow buffers (values, string offsets, validity bits), summed over the parts.
**A reader that budgets memory starts from `in_memory_bytes`**, never
`uncompressed_bytes`, and then applies its own multipliers: it is the size of the
Arrow buffers, **not an upper bound of what pandas holds**. What to add per type
(pandas 1.5 on numpy, per row):

| type | Arrow (`in_memory_bytes`) | pandas / Python |
|---|---|---|
| `int` | 8 bytes | 8 bytes as `int64`; 8 as `float64` if it has nulls |
| `float` | 8 bytes | 8 bytes |
| `bool` | 1 bit | 1 byte (x8); an `object` column of Python bools with nulls is 8 more |
| `date` | 4 bytes | 8 bytes as `datetime64[ns]` (x2) |
| `timestamp` | 8 bytes | 8 bytes as `datetime64[ns]` |
| `string` | 4 bytes of offset + the text | 8 bytes of pointer + about 49 bytes of header + the text per distinct Python `str`, so roughly 57 + the text (a dictionary read can share them) |

The Python side's memory budget must apply these multipliers itself; the manifest
does not.

**Versions.** The manifest has `version` 2 (version 1 had no `in_memory_bytes`). A
reader that sees any other version must refuse it, treat the source as not
prepared and ask for a new preparation; it must not guess a layout. The registry's
`FORMAT_VERSION` (`registry.rs`) is a separate number: a row written by an older
value is claimable again, and a manifest that cannot be read is demoted through
`mark_manifest_missing`. Whether the manifest version should also bump
`FORMAT_VERSION` is left to the unit that wires the manifest to the registry.

**Table list.** The same table list is what the registry row keeps as
`tables_json`, capped at 64 KiB and **never truncated**: a source whose table
list is larger fails (`ManifestTooLarge`) and no manifest is written. A manifest
read back from storage is validated (size before parsing, version, at most 256
tables and 16,384 columns per table, unique clean table names, unique non-empty
column names without control characters and at most 128 characters, at least one
part and no more parts than rows, `in_memory_bytes` at least the fixed width of
the type times the rows, no unknown fields), and the same validation runs before
a manifest is written, so anything that can be written can be read back (a
property test with generated manifests).

Parts live at `t<table>/part-NNNNN.parquet`, built by one function from numbers;
`parse_part_path` accepts only that exact spelling, so a key from outside is
never trusted as a part path. Excel sheet names that repeat (ignoring case) or
are empty or too long become unique, deterministic names (`Sales`, `sales_2`;
`sheet1` for an empty name).

### Part sink (`part_sink.rs`)

The `PartSink` trait takes one buffer with a known length per path (`put`), so
the host decides the storage and the converter never streams an unknown length.
Putting the same path again replaces it, which is what lets a restarted
conversion overwrite its earlier parts. `DirSink` is the local adapter used by
tests and the manual bench: it accepts only a canonical part path or
`manifest.json` (anything else is refused before the filesystem is touched) and
writes through a temporary file and a rename, so a failed write leaves no partial
file under a final name.

### Part writer (`writer.rs`)

`PartWriter` writes the Arrow batches of one table through a `PartSink`: ZSTD
level 3 (pinned on the writer properties, since a Parquet file records the codec
but not the level), dictionary encoding on, chunk statistics and exactly one row
group per part. A part closes at `PART_MAX_ROWS` (500,000) rows, or once its
encoded size reaches `PART_MAX_BYTES` (64 MiB), checked after each slice of at
most 8 MiB: an oversized batch is cut into slices, never encoded whole, so a part
is below the limit plus one slice and the writer never holds more than one part.
Only the column types the converter produces are accepted (`Int64`, `Float64`,
`Boolean`, `Utf8`, `Date32`, microsecond timestamp without a zone) and column
names must be unique, non-empty, clean and at most 128 characters; both are
checked when the writer is built. Each part is built in memory and handed over as
one buffer.

`finish` returns the rows, the parts and, per column, the type, the Parquet size
and the decoded size in memory (`in_memory_bytes`) summed over the parts; a table
with no rows still writes one empty part. Any error (a sink failure, a batch with
another schema) poisons the writer: it accepts nothing more and `finish` refuses,
so a failed table is never reported as written. `attempted_paths` lists every part
handed to the sink, including one whose put failed (`finish` takes `&mut self`,
so it is still readable when the last put fails), so the caller can remove them.

Encoding and compressing run on the blocking pool (a slice is tens of
milliseconds of CPU), never on the async worker: a test on a single-threaded
runtime counts how often another task runs while 8 MiB are encoded and flushed
(zero when either runs inline). A panic or cancel of that task is a typed
`WriterError`.

Dependencies for the writer and the rest: `parquet` 58.3.0 (only the `arrow` and
`zstd` features, no snappy, brotli, lz4 or async readers) and `arrow-array`,
`arrow-schema`, `arrow-cast`, `arrow-csv` 58.3.0. Arrow 58.3.0 was already in the
lock through another dependency, so the lock gains `parquet` and its few helpers
only; `encoding_rs` is used to read Windows-1252.

### Type inference (`infer.rs`)

The type of a column is decided from its first 10,000 rows; later rows are never
examined (a later cell that does not fit is handled by the conversion, which
demotes the column to text and starts over). An empty cell is null and does not
decide the type; a column with nothing to go on is text. Integers (`-?digits`)
and floats (`1.5`, `1e5`) are recognised only in plain form, at most 15 digits
(what a float holds exactly), and an integer with a leading zero (`00123`, a zip
code or an id) stays text. A float whose literal would underflow to zero or to a
subnormal (`1e-400`) is text. `-0` is an integer with the value 0. Booleans are
`true`/`false` in any case; dates are `YYYY-MM-DD` and real calendar days;
timestamps are `YYYY-MM-DD[T ]HH:MM:SS` with digits only in the clock and up to
six fraction digits and **no zone** (a cell with `Z` or an offset stays text
instead of being shifted). Integers with floats mix into floats; every other mix
is text. A test feeds the inferred schema to the Arrow CSV reader to prove it
parses everything the inference accepts.

### Record scanner (`scan.rs`)

The record scanner sits between the decoded text and the CSV parser. It follows
the parser's own quoting rules (a quote opens a quoted field only at the start of
a field, `""` is a quote inside one, a quoted field may hold line breaks), so it
knows where a record really ends. A record longer than 1 MiB (`MAX_RECORD_BYTES`,
quotes and delimiters included) fails with `CsvError::RecordTooLong { record,
limit }`, naming the record and never its content. This holds inside an
unclosed quote (`"12 inch pipe` makes the rest of the file one quoted field
although every physical line is short), in the header, in the sample and after
it; a line-based limit did not, and a parser would have buffered the whole file.

Blank lines are never skipped silently: in a one-column file (no delimiter
evidence in the sample) a blank line after the header is a null row (the empty
line is the empty cell; trailing blank lines at the end of the file and blank
lines before the header are dropped); in a file with several columns a blank line
is dropped. Every blank line is counted in `ScanStats` (`blank_rows`,
`blank_dropped`), and the output does not depend on how the input is chunked (a
test splits it at every byte offset).

### Reading a CSV (`csv.rs`)

`prepare_input` turns a blocking byte source into clean UTF-8 text and finds the
delimiter. It reads a 1 MiB sample, then: a UTF-8 byte order mark is removed;
UTF-16 and binary input (a NUL byte) are refused with a typed error; the
delimiter is the one of comma, semicolon, tab and pipe whose records (quote-aware,
the first 50, without the one the sample cut) agree most on a field count above
one, with a comma as the fallback. An empty file, or one with only whitespace, is
`CsvError::Empty`. Nothing past the sample is read before the consumer asks, and a
record is bounded by the record scanner, not by physical lines.

**Encoding rule.** An invalid UTF-8 sequence is never an error and never decides
the encoding by itself: it is replaced by U+FFFD and counted (`DecodeStats::invalid`,
with `valid_multibyte` the count of valid non-ASCII characters), so one stray byte
in a UTF-8 file costs one character and is reported instead of turning every
accent into mojibake. The content is *plausibly UTF-8* when there is no invalid
sequence, or at least half as many valid multibyte sequences as invalid ones (ties
and near-ties go to UTF-8: a counted replacement character is visible, mojibake is
not). Real Windows-1252 text has almost no valid UTF-8 multibyte sequences (an
accented letter is one byte), so its valid count is near zero against many invalid
ones. The rule is applied to the **whole file**, symmetrically: the 1 MiB sample
makes only the first guess, both encodings count the bytes as UTF-8 while they are
read, and at the end of a conversion run the whole-file counts decide; if the run
used the other encoding the file is read again, forced, once. So ASCII plus one
stray byte in the sample followed by UTF-8 accents is read as UTF-8 with one
replacement, and a real Windows-1252 file stays Windows-1252.

**Batches.** `open_csv` reads the header (row 1) and a sample to decide the types
(`infer.rs`): the first 10,000 rows, or fewer if 16 MiB of text or 2,000,000
fields are reached first (very long or very wide rows), and hands the rows back,
the sample included, as batches of text: empty cells are null, every other cell is
verbatim (`00123` stays `00123`, quoted delimiters and line breaks are kept). A
short row is padded with nulls and counted (`ScanStats::padded_rows`); a long row
is a `Parse` error naming the row. A batch holds at most 8,192 rows, 1,000,000
cells **and 8 MiB of text**, whatever the row size: a record that would pass the
budget starts the next batch (a record is at most 1 MiB, so one always fits), so a
file of 128 KiB rows gets batches of about 64 rows, not 8,192. Columns are built
with the exact capacity their values need, and the records kept (the sample and
the batch being built) are copied to buffers of their exact size, because the
parser's own buffers grow by doubling and a plain clone would keep up to twice
the text. The limits are the fields of `ReadLimits`, which only this crate can
name (`open_csv_with` and `convert_csv_table_limits` are crate-private): tests
move the sample and batch boundaries within a small file, and production code
always reads with the constants. Every limit is checked on opening (at least one,
at most its constant, and a batch holds a row of this file's columns); a bad one
is `CsvError::InvalidLimits`, and a batch that fits no row is the same error,
never the end of the data, so no limit can give an empty table that looks
successful. A file of more than 16,384 columns is refused (`TooManyColumns`). Header
names are cleaned by `column_names`: control characters become `_`, names are cut
at 128 characters, an empty one becomes `column<N>` and repeats get `_2`, `_3`
(compared as written), in work linear in the number of columns (each base name
remembers its next suffix; 16,384 identical headers take 16,384 probes, not 134
million). A failure of the source (a guard's typed error) comes out of the
iterator as that typed error, and the iterator ends after it.

### Conversion (`convert.rs`)

**Typing.** `TypedBatches` turns the text batches of `open_csv` into typed Arrow
batches for a given schema (the inferred one, or one with columns the caller
demoted to text). Each cell is checked by the same rules that chose the type
(`infer::cell_fits`), not by what a number parser would accept: a late `007` in an
integer column, `+5`, `1e3` or `yes` in a boolean column is a conflict, not a
changed value. The first cell that does not fit ends the iteration with
`ConvertError::Conflict { column, row }` (row counted from zero over data rows;
with several in one batch, the earliest row), after the batches before it were
delivered typed. After a column is cast, the stored values are verified against
the cells: every non-empty cell must be a non-null value (Arrow's cast turns what
it cannot parse into null) and a float must be exactly the value of its literal; a
disagreement between the inference and the cast is a conflict, never a changed
value. A test feeds 200,000 generated near-miss strings to every type and asserts
each is either refused or stored as its own value.

`stream_reader` reads a storage stream (`read_stream`) as a blocking source for
`open_csv`: build it inside the runtime and read it on a blocking thread; a
storage error becomes an `io::Error` carrying its text. A read waiting on a
stalled stream is woken by the conversion's cancel token and fails with
`CsvError::Cancelled`; what cannot be interrupted is a read stuck inside a
stream's own non-async code (none of this module's readers).

**The pipeline.** `convert_csv_table` runs the whole conversion of one CSV into
the parts of a table, from a source that can be opened (`CsvSource`) to a
`PartSink`. Reading, parsing and typing run on a blocking thread and the writer on
the async side, joined by a channel of two batches, so memory is bounded by the
reader's batch limits plus one part, however large the file (the arithmetic is in
the `convert` module documentation: a ceiling of about 310 MiB at the default
settings, against the 4 GiB of the job; wide tables and the sample included). An
integration test measures the peak of live heap bytes with a counting allocator on
a 270 MB file (55 MiB), on 700 columns (96 MiB), and on files of 3,000 and 16,384
columns that stop after the sample (32 and 39 MiB); each scenario has its own
bound, the measured peak plus a margin, so the slack that was removed would fail.
Any failure (a cell that contradicts its type, a bad source, a sink error) returns
the paths that may exist in the sink (`blob_paths`) and nothing is reported as
written; a reader that dies midway is a failure, never a short table (a panic in
it is `ConvertError::ReaderPanicked`), and on any failure the reader is cancelled
and joined, so a failure is reported even when the reader is parked on a stalled
stream.

**Restarts.** The types come from a sample, so a late cell can contradict them:
the column that conflicted becomes text and the file is read again from the start.
At most **three** restarts: the first two demote the column that conflicted, the
third makes every column text (which cannot conflict), so a file costs at most four
reads for types, plus one more if the whole-file counts say the other encoding was
right (five in the worst case, which a test pins). Parts are written under
deterministic keys, so a restart overwrites what the run before wrote; `blob_paths`
lists every part key any run handed to the sink, each once. The exposed schema
shows the fallback type: `demoted` lists every column that was typed from the
sample and ended as text (all of them when `all_strings` is set).

**What the result reports**, so nothing changes silently: `encoding` and
`replacements` (invalid UTF-8 sequences replaced; zero for Windows-1252),
`utf8_valid_multibyte` and `utf8_invalid` (the whole-file counts the encoding
decision used, for both outcomes), `blank_rows` (blank lines kept as null rows,
one-column files), `blank_dropped` (blank lines dropped) and `padded_rows` (short
rows padded with nulls).

**Using the encoding evidence.** The decision rule is not changed by the counts; a
caller uses them to tell the user when to doubt it. Warn that text was altered when
`replacements > 0` (UTF-8: that many characters became U+FFFD). Warn that a file
that looks like UTF-8 was read as single bytes when `encoding` is Windows-1252 and
`utf8_valid_multibyte > 0`: the accents may be mojibake. Warn that the choice was
close when both counts are non-zero and neither is ten times the other. Two misreads
are known, and both leave this evidence: a Windows-1252 file whose accents are an
uppercase letter followed by a byte 0x80-0xBF (`Ã©` is the bytes C3 A9, valid UTF-8
for `é`) is read as UTF-8 with `utf8_valid_multibyte > 0` and no invalid sequence,
which is indistinguishable from real UTF-8 by the bytes alone; and a UTF-8 file with
many sequences cut mid-character (more than twice as many cut ones as whole ones)
is read as Windows-1252 with a large `utf8_invalid` next to a non-zero
`utf8_valid_multibyte`.

**Table list cap.** A table list that cannot fit the registry row (64 KiB) fails
right after the header and sample, not after the whole file. The check uses the
smallest table list the columns could have (the shorter of each column's two
possible type names, the least decoded size the rows of the sample allow, zero
stored bytes), a true lower bound, so a file that would fit is never refused. It is
tight to within about 2% (a 200-column table: 14,937 bytes against 15,137
written); what stays undetected until the end is a table list within that band of
the cap (about 860 to 880 columns with short names), because the digits of the
stored sizes are only known then. A table wider than about 900 columns therefore
never reaches a batch.

**Stale parts.** A restart that ends with fewer parts than the aborted run left
behind (or a source that changed between reads) leaves keys that no manifest
references. `ConvertedTable::live_paths` lists the parts of the finished table,
`stale_paths` the rest of `blob_paths`, so a caller can delete them; each result
lists only the keys under its own `t<idx>/`, so a control shared by several tables
never reports one table's parts as another's; the manifest refers only to live
parts, and **a reader must take parts from the manifest (`parts`), never by
listing the prefix**.

**Cancellation.** `convert_csv_table_with` takes a caller-owned `ConvertControl`:
every part key is recorded in it *before* its put, so after the future is dropped
(a timeout, a cancelled request) the caller still reads `control.paths()` and
removes those keys from the sink; dropping the future or calling `control.cancel()`
cancels the reader (it fails at its next read, or at once if waiting on a stalled
stream) and the run ends with `Cancelled`. A `CsvSource::open` receives the token to
hand to `stream_reader`. Wait for the conversion future to finish (or drop it)
before deleting anything: a conversion still running can put a key you just
deleted, and `cancel()` stops the reader but does not interrupt a put in flight (up
to two batches already read can still be put after it).

**Tests of the whole.** The same table with the input split at every byte offset of
the pipeline's chunks (the tricky part is moved across the 16 KiB grid by a filler
row), with random read sizes over a 1.5 MiB file, the fragment moved row by row
across the sample, batch and part boundaries (`ReadLimits`) and compared with rows
written by hand, and a value-by-value read-back of every type across four parts.

### Preparing a source (`prepare.rs`)

**Layout.** Everything a source produces lives under the root its storage adapter
reports with `derived_root`, and nowhere else: `<root>/manifest.json` and
`<root>/t<n>/part-NNNNN.parquet` (the table index has at most four digits, the part
number five; the ADP signing route accepts exactly this pattern). `StoragePartSink`
builds each key as `<root>/<relative path>` before the put, hands the object to
`store_stream` with `StorePlacement::DerivedFrom`, and refuses the answer when the
adapter returns any other key: the cleanup pass deletes only keys it can contain in
the root, so an object stored elsewhere could never be removed. An adapter that
reports no root (or an empty one) cannot be used: preparing refuses to start and
writes nothing. The default adapter of this crate ignores the placement, so it is
refused too; the first adapter that honours it is the host's.
Errors never echo a storage key, a URL or a cell: the adapter's text is dropped and
replaced by a fixed sentence.
