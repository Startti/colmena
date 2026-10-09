# 54. Tabular prepare (large CSV/Excel preparation)

Module `tabular_prepare` will hold the pieces that prepare a large tabular
attachment once, ahead of the questions asked about it. Everything in it is
**dark**: it is used only when the engine switch `COLMENA_LARGE_TABULAR=on`,
and nothing calls into the module yet (the CSV and xlsx converters exist, see
below, but no host wires them). With the switch off
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

The lease is a single fixed value, with no renewal: the registry is written on state
change, not for progress. The registry's `lease_for` is `P_PREP_TIME + 60 s`; the CSV
driver claims with the budget plus its own 90 s grace (see *Time budget* below).

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
A cancelled job deletes what it wrote. The CSV driver lists each key in the row
before it writes the object (`track_blobs`), so the blobs of an attempt that
crashed before any terminal write are tracked too; a host that can delete by
prefix also covers a writer that did not list its keys.

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
it and thereby also removes blobs nobody tracked, such as those of a writer
that did not list its keys). Nothing calls them yet: the cleanup
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
`mark_manifest_missing`. The two move together: `FORMAT_VERSION` is 2, the
manifest version, and a compile-time assertion keeps them equal, so a table
prepared under the first layout (version 1) is claimed again by the current one
(its old objects stay tracked for cleanup).

**Conversion report.** The manifest may carry a `conversion` array, one entry per
table, with what the conversion did and a reader or the tool should warn about:
`encoding` (`utf-8` or `windows-1252`), `replacements`, `utf8_valid_multibyte` and
`utf8_invalid` (the whole-file evidence of the encoding choice), `blank_rows`,
`blank_dropped`, `padded_rows`, `restarts`, `all_strings`, and `demoted_count` with
the first 32 `demoted` column names. It is optional (a manifest without it is
valid and carries no `conversion` key) and it is not part of `tables_json`, which
the registry row keeps and which stays within its 64 KiB cap however many columns
a table has. A manifest that would be larger than the 128 KiB a reader accepts is
refused when it is written, never written unreadable. A reader that does not know
the key must ignore it only if it parses the manifest loosely: this crate's own
parser refuses unknown keys, so a reader of another version needs a version bump.

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
`StorageCsvSource` reads the source with `read_stream` as a blocking stream (a
missing source is `ConvertError::SourceMissing`, anything else
`SourceUnavailable`) and counts the bytes read, which is what progress reports.
Errors never echo a storage key, a URL or a cell: the adapter's text is dropped and
replaced by a fixed sentence.

**The driver (`driver.rs`).** `PrepareEnv` holds what a preparation needs: the
registry, the storage, a clock, the time budget (`PREP_TIMEOUT`, 300 s) and the
writer settings. A preparation ends in a `PrepareOutcome`: `Ready` (a
`PreparedTable` with the manifest, its key, every key any attempt may have
written, the stale ones, the prepared bytes and everything the conversion
reported), `Refused` (no derived root: nothing written, not even a row),
`NotClaimed`, `Cancelled` (the row is gone or another job owns it: nothing
recorded) or `Failed` with a reason. The table is named after the file without its
extension (cleaned by `unique_table_names`), and the manifest records the
conversion report of the table.

`prepare_csv` claims the registry row (with `FORMAT_VERSION`, a lease of the time
budget plus `JOB_GRACE`, 90 s), converts the source as table 0 with the part sink above, stores
the manifest LAST and completes the row with the manifest key, the table list and
the prepared bytes. The row's `blob_keys` are the union of every key any attempt may
have written (listed in the row before each put, see *What holds about the objects*),
as full storage keys, so a failed put and a restart that wrote more parts than
the final run are both tracked. The manifest names only the parts of the final run;
the others come back as `stale_keys` for deletion and are never referenced. A
failure is handled in this order: ownership is read, the objects are deleted (best effort:
they are listed, so the cleanup pass removes what this could not), and only then is the
failure recorded with `fail_with_blobs`. Nothing is deleted after the write: once the row
is `failed` its lease is cleared and a retry may claim it at once and write the same
deterministic keys.
A registry that cannot be written is returned as an error: nothing could be recorded.

**Time budget.** The conversion runs against `PrepareEnv::budget` (`PREP_TIMEOUT`,
300 s). When it ends first the run is dropped (which cancels its reader), the keys it
recorded before each put are read from the control, the objects are deleted and the
failure is then recorded as `time` with all of them. The wait goes through `PrepareEnv::sleeper`
(`tokio::time::sleep` by default), which is what the tests replace: no test depends on
how long anything takes.

The budget covers the whole job up to the manifest: one deadline, taken once, bounds the
conversion and the manifest put. What comes after it is bounded step by step: the claim,
the terminal writes, the ownership reads, the deletes and the progress reports each get
`TERMINAL_STEP` (10 s) and are given up on, never waited for forever (a registry step that
does not answer is an error with a fixed text; a delete that does not is logged and left to
the cleanup pass). The lease is the budget plus `JOB_GRACE` (90 s), not the registry's 60 s
grace: the longest path an owner can take outside the budget is seven bounded steps (the
claim and the first progress report before it; after it, for a source found missing whose
cleanup fails, an ownership read, a delete, an ownership read, a delete and the failure
write), 70 s at 10 s each. A compile-time assertion keeps that shorter than the grace, and a
test runs that path for real on a virtual clock, every step taking its whole bound and the
conversion the whole budget: it checks the list of steps and that every delete and terminal
write happens before `lease_until`, so adding a step to the path fails it. A job that no
longer owns its row writes nothing that needs the lease, so its steps are not counted.

**Failure reasons.** A failure is recorded in the registry with a reason (`error_code`)
and a fixed detail sentence (`error_detail`); neither carries a cell, a storage key or
the adapter's text. The set is small and stable, lowercase snake_case, and the host
maps it to what the user sees:

| reason | written when |
|---|---|
| `time` | the time budget (300 s) ran out; the partial output is removed |
| `storage` | the source could not be read from storage, or a part or the manifest could not be stored, or the adapter answered with a key outside the prepared layout |
| `unreadable_file` | the file cannot be read as CSV: empty, UTF-16 or binary, a row longer than the header, a record over 1 MiB, more than 16,384 columns, or text that does not parse; or as xlsx: not a zip at all (also a truncated one), no workbook part, a bad relationship, XML that does not parse, a cell, row or XML element over its limit, a value past the last column of the header, or no sheet with data |
| `table_too_large` | the table list does not fit the 64 KiB registry row (too many or too long column names) |
| `internal` | anything else (a reader that panicked, a conversion that stopped unexpectedly, the registry unable to list keys or confirm the row): a defect or an outage on our side, never the file's fault |
| `output_too_large` | the parts stored passed 1 GiB in all (see below); the detail says to export fewer rows or columns |
| `xlsx_too_large` | a workbook over a size limit: more bytes than `PrepareEnv::xlsx_max_bytes` (400 MiB; checked against the size the host declared **before a byte is read**, and again against the bytes as they arrive), more than 256 sheets, 1,048,576 rows or 16,384 columns in a sheet, 50,000,000 cells, or more shared-strings text than 128 MiB. The detail says to export as CSV |
| `archive_limit` | a workbook whose zip archive is over a safety limit or inconsistent: a zip bomb (an entry or the whole over its expanded size, a compression ratio under 1 %, sizes deflate cannot produce), too many entries, an unacceptable or repeated entry name, zip64 or encryption, headers that disagree, or a part that inflates past the size its header declares |

**The two Excel reasons are spelled with an underscore** (`xlsx_too_large`, `archive_limit`).
The ADP status endpoint currently accepts both `archive-limit` and `archive_limit`; it has to
be reconciled with this spelling later. The detail sentences carry no sheet name, cell, key or
library message: they are chosen by the kind of error alone (a test checks each one), so a
sheet over a cap is named by the cap (`a sheet has more than 1048576 rows`), not by its name.

Nothing is written for a cancellation (the source was deleted or another job owns the row).

**Ownership.** Every object a preparation writes (each part, then the manifest) is
preceded by an ownership check, a `still_owned` read, or the owner-guarded tracking write
at the start of each batch of sixteen parts and for the manifest: a preparation whose row was deleted
(the source was removed) or taken by another job stops before its next write and never
completes. The objects have deterministic keys, so a job that lost its row must not
write over those of whoever owns it now. What it does then depends on why: if the row is
gone, what it wrote belongs to nobody and is deleted; if another job owns the row, the
objects are left alone, and the registry is not written either way (`Cancelled`). A
`complete` that finds the row gone or taken is the same case. A source that does not
exist (on the first read, or on a restart after some parts were written) deletes what was
written and releases the row, keeping no failure (`SourceGone`); if the objects cannot be
deleted the row stays, failed with `storage` and the keys, so the cleanup pass can reach
them. A storage that cannot be reached is not a missing source: it is a `storage` failure.
The part bookkeeping is per table, so the first part of a second table is listed before
it is put.

**Progress.** While a preparation runs it reports `PrepareProgressInfo` to the host's
progress port (`PrepareEnv::progress`, the `NoopProgress` by default; build the
environment from `PrepareConfig::progress`): `Running` when it starts, then every 2 s
(`PROGRESS_INTERVAL`) with the bytes of the source read so far over the declared size
(never above it, whatever the restarts re-read), and a final `Ready`, `Failed` or
`Cancelled` state. Progress never touches the registry: the row is written only on
claim and on the terminal write. Nothing is reported for a source that was not claimed
or refused.

**Inline runner.** `CsvPrepareRunner` is the `PrepareRunner` behind `InlineTrigger` for
local runs and tests: built from a `PrepareEnv` and the `PrepareConfig` (it keeps the
engine switch). With the switch off it runs nothing at all (no registry read, no storage
call). A request whose mime type is `text/csv` is prepared by `prepare_csv` and one whose
mime type is the xlsx one (`application/vnd.openxmlformats-officedocument.spreadsheetml.sheet`,
`XLSX_MIME`) by `prepare_xlsx` (parameters such as a charset are ignored, case too); any other
mime type, legacy `.xls` included, is logged and dropped. The host that wires production
storage and registry builds the same environment and its own trigger; nothing here constructs
either.

**Tracking before writing.** `PreparationRegistry::track_blobs(source, owner, keys, now)`
adds keys to the row's `blob_keys` (a union), only while `owner` holds the lease of a
`running` row; `Cancelled` means the caller must not write. It does not renew the lease and
it is not progress: it is the write that makes it true that every object a preparation may
have written is listed in the row, whatever happens to the process afterwards.

**What holds about the objects.** Before an object is put, its key is listed in the row
with `track_blobs` (owner-guarded): the part keys are deterministic, so one write lists the
next sixteen parts and the manifest, and a table of up to sixteen parts costs one tracking
write on top of the claim and the terminal write. So every object a preparation may have
written is in the row whatever happens next, a crash, a dropped future or a registry error
at the terminal write; a key listed and never written is harmless (deleting is idempotent,
and a ready row of three parts also lists the thirteen unused part keys, which cleanup
deletes as no-ops). If the failure cannot be recorded the objects are already deleted (they are deleted first); if
`complete` errors, whether it was applied is unknown, so nothing is deleted and the objects
stay listed. If the row is gone and the registry errors when the job asks (`get`), what it
wrote is left behind with no row to list it: the one case the row cannot cover.

Every terminal path makes the same decision when its write finds the row not ours: a
budget that ran out, a storage or source failure, a manifest that could not be written, a
completion and a source found missing all end in "row gone, delete what this job wrote" or
"row taken, delete nothing and release nothing". A source found missing checks ownership
before it deletes or releases.

**Logs.** The driver and the runner log a fixed sentence, a `kind` or a `reason`, and an
opaque `source` identifier (the first 12 hex digits of the SHA-256 of the source key: it
follows one preparation through the logs and cannot be turned back into the key). They
never write a storage key, a URL or the text of a registry or storage error; a test runs
the failing paths with error texts that name a key and checks none of it appears.

**Known limits, stated.**

- *A source the adapter calls `InvalidInput`.* `StorageError::InvalidInput` from
  `read_stream` is how the adapters say "no such object", so it releases the row (and resets
  its attempts) like a deleted source. A key that can never be a key (empty, over 1 KiB, a
  control character, a `..` segment) is refused before the storage is asked, and writes
  nothing; an adapter that answers `InvalidInput` for some other reason would make a
  re-triggered preparation start over each time, unbounded by the attempt cap. The trigger
  is idempotent and a preparation costs one read of the file at most, so this is tolerated
  rather than bounded.
- *Check then write.* Ownership is read (or written, at a batch boundary) before an object
  is put, and the put itself is not conditional: a lease taken between the check and the put
  lets one object be written over the new owner's key. The lease outlasts the owner's whole
  bounded job (see *Time budget*), so a takeover cannot follow an expiry; it can only follow
  a deleted row that someone claims again within that put's duration. If that new claim is
  for the same unchanged source the bytes written are the same; if the source key was
  rewritten in between they are not, and the table could mix versions. The upload paths give
  every file a fresh key, so a key is not reused after a deletion; this is tolerated, not
  closed (it needs a conditional put the storage port does not have). The new owner's own
  terminal write is conditional on its own lease. When a job finds the row gone, the `get`
  that decides whether to delete is not atomic with the delete either, for the same reason.
- *A delete with no row.* When the row is gone the objects the job wrote are listed nowhere.
  A delete that fails or does not answer in its bound then leaves them (the default
  `delete_derived` stops at the first error): the log says so ("no row lists them") and they
  stay until a host that deletes by prefix, or someone, removes them. With a row (every other
  path) the keys are listed and the cleanup pass removes them.
- *Progress.* `done` is the bytes of the current read of the file and stays under the total
  until the table is ready; after a restart (a column that turned out to be text) it starts
  over.

## Excel (xlsx) preparation

An `.xlsx` above 50 MiB is prepared into the same layout as a CSV
(`<derived_root>/manifest.json`, `<derived_root>/t<n>/part-NNNNN.parquet`, one table
per sheet) by the same driver. The unit is built in slices and this section grows with
each one. Everything is dark behind `COLMENA_LARGE_TABULAR`, like the CSV.

### Archive pre-check (`precheck.rs`)

An xlsx is a zip. Before any entry is opened, `check_archive` reads the
end-of-central-directory record, the central directory and the local header of every
entry, and refuses the archive against limits. **Nothing is inflated**: the reads are
the last 64 KiB of the file, a central directory of at most 8 MiB, and 30 bytes plus a
name (at most 512 bytes) per entry, so a bomb costs a few small reads whatever it would
expand to (a test counts the bytes read over a 20 MiB entry).

| Limit | Value | Provenance |
|---|---|---|
| entries | 10,000 | estimate, not prototyped |
| central directory | 8 MiB | estimate, not prototyped |
| one entry, uncompressed | 2 GiB | spike item 6: the largest real entry at the cell cap is 1.74 GiB |
| all entries, uncompressed | 2.5 GiB | spike item 6 |
| ratio, entries above 1 MiB | compressed at least 1 % of uncompressed | spike item 6: lowest real workbook 8.2 %, bomb 0.097 % (generated files only) |

Also refused: sizes deflate cannot produce (more than 1032 times the compressed size;
a stored entry whose two sizes differ), a name that is not UTF-8, is empty, is longer
than 512 bytes, has a control character or a backslash, starts with `/`, starts with a
drive letter, or has a `..` segment, two entries with one name, a local header that
disagrees with the central directory (method, flags, CRC, sizes, name; with a data
descriptor the local CRC and sizes are zero by design and are not compared), an entry
whose bytes run outside the file or into another entry, and zip64, encryption, several
disks or a method other than stored and deflate (`Unsupported`: no workbook under the
caps needs zip64, whose sizes start at 4 GiB). The central directory is what the reader
trusts, so a local header that says something else is a lie about the size.

Errors are a fixed enum (`ArchiveError`) whose text carries no name, size or byte of
the file. A file that is not a zip at all (`NotAnArchive`, also a truncated one) is told
apart from one over a limit: the driver will record the first as `unreadable_file` and
the rest as `archive_limit`.

### Spooling the source (`xlsx_spool.rs`)

**How it is read.** The storage port only streams a source from its start and a zip keeps
its directory at the end, so the workbook is **spooled to a local temporary file** (at
most 400 MiB, `MAX_XLSX_BYTES`; the storage's declared size is checked before a byte is
read and the bytes are counted as they arrive, so a size that lies is caught) and read
from there with random access. Memory is a chunk of the stream, never the file. On Unix
the file is unlinked as soon as it is created (mode 0600), so it cannot outlive the
process; elsewhere a guard removes it. **A temporary directory can be memory-backed (Cloud
Run's is), in which case the 400 MiB count against the job's memory.** A source that does
not exist, one that fails and a cancel are told apart (`SourceMissing`,
`SourceUnavailable`, `Cancelled`); a local disk failure is `Local` (an internal failure,
never the file's fault). Over the cap is `TooLarge(Bytes)`, which the driver will record
as `xlsx_too_large`.

`XlsxError` is the one error of the xlsx reader: the archive pre-check's errors, the caps,
a part that is not valid, and the source and local failures above. Its text is fixed: no
name, key, cell or library message is echoed.

### Reading parts as streams (`xlsx_package.rs`)

`Package::open` runs the pre-check and opens the archive; `Package::xml(name)` returns a
part as an XML reader under **two guards**, because the pre-check only reads headers:

- `Limited` ends a part at the size the archive declared for it. Deflate can produce more
  than a header says (a consistent lie in both headers passes the pre-check), so a byte
  past the declared size is `EntryTooLarge`. Together with the pre-check's caps this bounds
  everything that is inflated: at most 2.5 GiB over a whole workbook, 2 GiB per part.
- `Guarded` bounds what the XML parser buffers. The parser copies a text node or a tag
  whole before it hands it over, so a single 1 GiB text node would be 1 GiB of memory. The
  guard fails the read once more than 1 MiB (`MAX_TOKEN_BYTES`, the CSV record limit) was
  consumed since the last event, whatever the structure (CDATA and comments too), and the
  parse loop resets it after every event. A test shows the parser buffering less than the
  limit plus one 16 KiB buffer of a 50 KiB node.

**What the parser holds, whatever the XML.** `Guarded` resets per event, so it bounds one
token but not the elements left open: the parser keeps the name of every open element until it
is closed (about nine bytes of bookkeeping for a three-byte `<a>`), so millions of unclosed
elements, padded to pass the ratio rule, would be gigabytes. Every part read goes through
`next_event`, which therefore caps the **nesting depth at 32** (the deepest part read, a
rich-text shared string, is seven) and **an element name at 256 bytes**: what the parser holds
for open elements is at most 32 x 256 bytes. `attribute()` caps **the attributes of a tag at
64** and turns off the parser's duplicate-attribute check, which compares each attribute with
every earlier one (about 2 x 10^8 comparisons for 20,000 attributes in one 1 MiB tag, repeatable
for every tag of a part). Each kind of part has a **declared-size cap of its own** instead of
the 2 GiB per-entry limit: 16 MiB for the workbook and its relationships
(`MAX_SMALL_PART_BYTES`; what is kept of them is a few strings), 64 MiB for the styles
(`MAX_STYLES_PART_BYTES`: real workbooks with style bloat have tens of megabytes, the part is
parsed as a stream and what is kept is one byte for each of at most 65,536 styles and a map of at
most 65,536 custom formats, about 4 MiB, so the cap bounds only time), 128 MiB for the shared strings, and for a sheet the per-entry limit,
2 GiB (`MAX_SHEET_PART_BYTES`), inside the job's running budget. `tests/xlsx_hostile_memory.rs`
reads 3,000,000 unclosed elements as each kind of part and asserts the refusal (`TooDeep`)
costs under 2 MiB.

**Elements only where they belong.** A `row` opened inside an open `row` reset the row's byte
and column counters while the cells it held stayed, so both per-row caps could be bypassed and the
cells grew without bound (a 128 KiB shared string behind 8 cells, then an empty `row` element, again
and again: a megabyte of memory per 250 bytes of XML), and the same construction delivered column
indexes out of order to the header builder, which indexed past its width. The sheet reader now
tracks where it is (`Outside`, `sheetData`, `row`, `c`): `sheetData` opens only outside, `row` only
in `sheetData`, `c` only in a `row`, and a row closes only from `row`; anything else, and a part that
ends inside a row or a cell, is `BadCell` (an unreadable file). Cells arrive at a row's end, so the
row caps cannot be reset while cells are held, column indexes are strictly increasing
(`begin_cell` refuses a repeat or a step back), and the header builder cannot panic whatever order
it is given. The other readers were audited for the same class (state reset on an element start that
can be re-entered): in the shared strings a `si` inside a `si` is now refused (it would add strings
the table did not count); the styles, workbook and relationships readers keep no state that an
element start resets (their counters and maps are capped, and only grow). Every part read also
looks at the cancel token every 4,096 events, so a part of millions of comments or of elements
that are not cells stops within a bounded number of events.

**Budgets that cover every read.** A part read again is inflated again (a sample, a run, each
restart). A flat running total of inflated bytes would refuse a legitimate sheet of over 1.25 GiB
of XML that needs one restart, so the limit is on **reads**: a part may be opened at most five
times in a job (`MAX_READS_PER_PART`: the sample, the run and the three restarts of
`MAX_RESTARTS`), each read bounded by the size its header declared (`Limited`) and the archive by
the pre-check's 2.5 GiB. Worst-case CPU: five reads of a part, so at most 5 x 2.5 GiB of XML to
parse in all (a few minutes at 100 MB/s), which the job's 300 s budget ends first, because every
cell and every 4,096 events look at the cancel token. A workbook whose `sheet` elements name one
part more than once (up to 256 of them could, each read as another sheet) is refused
(`BadRelationship`). The cell cap works the same way: the book counts every cell any read of it
reads, and the reader looks at the cancel token every 4,096 cells, whatever they hold, so a sheet
of tens of millions of empty cells ends with the budget instead of outliving it.

**One reading of the archive.** The part is found at the offset the pre-check validated (its
local header already compared with the central directory) and only its raw bytes go to a
decoder (`flate2`, or none for a stored part), under `Limited`, which also checks the part's
CRC-32 at its end (a part shorter, longer or different from what the directory says is an
error). **No library ever discovers a directory of its own.** The first version handed the
file to `zip::ZipArchive`, which looks for the end record nearest the end of the file with
no check of its comment: a forged record placed in the real one's comment made it parse a
directory the pre-check never saw (65,535 entries, no entry cap, no size cap). Two refusals
close the class on the pre-check's own side too: the central directory must end exactly at
the end record (`cd_offset + cd_size == eocd_at`, no gap that could shift where another
reader finds it), and a comment that contains another end-record signature is refused.
`tests/xlsx_hostile_memory.rs` opens both crafted archives with a counting allocator and
asserts the refusal costs under 2 MiB.

Parts are looked up by the names the pre-check saw (unique, with no `..`), never by
listing.

**Dependencies.** `flate2` 1 (the pure-Rust backend) and `quick-xml` 0.31 are direct
dependencies, at the versions already in the lock (through `zip` and calamine), so
`Cargo.lock` gains only the two edges and no package; `zip` is now a dev-dependency that
builds workbooks in tests. Neither calamine nor `zip` reads a large workbook: calamine opens
the archive itself (no inflate guard) and loads the whole shared-strings table into memory,
and `zip` discovers its own directory (above).

### The structure of the workbook (`xlsx_workbook.rs`)

`read_workbook` finds the workbook part through the package relationships (`_rels/.rels`,
type `officeDocument`), never by a fixed name, then reads the `sheet` elements, the date
system (`workbookPr date1904`) and the relationships the workbook names. Every target is
resolved against the directory of its part and must stay inside the package (a target that
climbs out with `..` is `BadRelationship`) and name a part the archive has (`MissingPart`);
external relationships are ignored. Bounded by construction: at most 256 `sheet` elements
are read (a 257th is `TooLarge(Sheets)` and the rest of the part is not read; 256 is the
number of tables a manifest can hold), a sheet name is cut at 255 characters, and a
relationships part is scanned for the ids the workbook asked for, so what is kept is a
handful of strings whatever its size.

Decisions: a sheet whose relationship is not a worksheet (a chart sheet, a dialog sheet, a
macro sheet) has no cells and is not a table. **Hidden and very hidden worksheets are
kept**: the file is what the user uploaded, the manifest lists every table, and a hidden
sheet is not a reason to drop data silently (the manifest has no visibility flag, so the
model cannot tell it was hidden).

### Dates (`xlsx_styles.rs`)

A cell stores a number and its style says how to show it, so only the style tells a date
from a quantity. `read_styles` reads the number format of every `cellXfs` style (the
`cellStyleXfs` are not cell styles and are ignored); a style that does not exist is plain.
At most 65,536 styles and as many custom formats are read (Excel's own limit is 64,000);
a part with more is `TooManyStyles`.

- **When a number is a date.** Only when its style's format is one: a built-in date or time
  format (14 to 22, 27 to 36, 45 to 47, 50 to 58), or a custom code with a date or time
  letter (`y d h s`, `m` as a month, or a minute beside an hour or second) in its first
  section, outside quotes, brackets (a colour, a locale; `[h]` `[m]` `[s]` are elapsed time)
  and after a backslash or underscore. `General`, `0.00`, `0.0%`, `0.00E+00`, `@` stay numbers.
- **What a date serial becomes** (`temporal`): a date (`YYYY-MM-DD`; days since 1970), a
  timestamp (`YYYY-MM-DD HH:MM:SS`, microseconds, **no zone**) when the serial has a time or
  the format shows one (a date format over a serial with a time keeps the time, so a value
  is never reduced to its day), or a time of day (`HH:MM:SS`) for a time-only format or a
  fraction on day 0. Seconds are rounded; **a fraction of a second is not kept**.
- **The 1900 system keeps Excel's quirk**: serial 60 is a 29 February 1900 that never
  existed, so it stays a number, and serials 1 to 59 are shifted one day to keep the rest
  right (44,197 is 2021-01-01; tested at 1, 59, 60, 61 and 9999-12-31). The 1904 system
  (`workbookPr date1904`) is 1,462 days behind (42,735 is the same 2021-01-01).
- **No date means a number**: negative, not finite, past 9999-12-31, day 0 without a time,
  an elapsed time past one day (`[h]:mm` over 1.5), and any serial under a plain format.

### Shared strings (`xlsx_strings.rs`)

Cells point into the shared-strings table by index and a sheet can use any entry at any
row, so it is the one part of a workbook that is **read whole**. Hence a cap with a typed
refusal, not a spill:

- The part's declared size must be at most 128 MiB (`MAX_SHARED_STRINGS_XML_BYTES`; the
  archive pre-check already knows it, so a larger one is refused before it is read) and the
  table at most 10,000,000 entries; else `TooLarge(SharedStrings)`, which the driver records
  as `xlsx_too_large`.
- The text goes into one buffer and one `u32` end offset per string, both **reserved up
  front from the declared size**, so the table never grows by doubling: the text is never
  larger than the XML it came from (decoding only shrinks it) and the offsets are at most
  40 MiB. Worst case about 168 MiB, against the 4 GiB of the job, and reached only by a
  workbook whose sheets are already near the 400 MiB cap.

A real workbook near that cap can have more unique text than this holds; it is refused with
a message to export as CSV. **Spilling the table to the local file is the known way to lift
the cap and is not built.**

A string is the concatenation of the text runs of its `si` element (a plain `t`, or the `t`
of each rich-text run `r`, whitespace kept); phonetic runs (`rPh`) are not part of the value.
Entities and CDATA are resolved, and so is Excel's `_xHHHH_` escape (a carriage return is
`_x000D_`, `_x005F_` is a literal underscore) when it is exactly that.

### Reading a sheet's cells (`xlsx_sheet.rs`)

`read_sheet` parses a worksheet as it is inflated and hands it on **one row at a time**; only
the current row is ever held, however many rows the sheet has. A row arrives as its non-empty
cells with their column index (empty cells are not delivered), so a sparse row costs what it
holds, and the callback can stop the read. Rows with no value (absent, self-closing, or only
empty cells) are counted in `SheetStats::blank_rows` and not delivered.

**Types.** Shared, inline (rich runs joined, phonetic runs dropped) and formula strings are
text; a boolean is a boolean; a number is a number (a stored value that is not a number stays
text); an error (`#DIV/0!`) is its **text**, so it is visible and turns a numeric column into
text instead of vanishing; an empty string is empty, as in a CSV. Cells are counted as `c`
elements, empty ones too, because they cost the parser the same.

Two more decisions, each pinned by a test:

- **Formulas are never evaluated.** A formula cell is read as the value Excel cached beside it
  (`v`); one with no cached value is empty. The formula text (`f`) is skipped, never parsed.
- **Merged cells are not expanded.** Excel keeps a merged range's value in its top-left cell
  and leaves the others empty; that is what is read (`mergeCells` is ignored). Filling the
  range would invent values.

**Limits** (inclusive): 1,048,576 rows and 16,384 columns (Excel's own, and the CSV's column
cap), 50,000,000 cells (`TooLarge(Cells)`), 131,072 bytes of text in a cell (Excel's 32,767
characters at four bytes) and 1 MiB of text in a row (the CSV record limit): the last two are
`CellTooLong` and `RowTooLong`, which the converter reports as an unreadable file, like a CSV
record over its limit. A row number or a column that does not increase, or a shared-string
index that does not exist, is a corrupt part (`BadCell`).
- **Dates.** A number whose style's format is a date or time (see *Dates* above) is delivered
  as a date, timestamp or time (`Cell::Temporal`); the same number under a plain style stays a
  number, and so does one under a style that does not exist. An ISO 8601 cell (`t="d"`, a date
  or a date and time without a zone) is a date or timestamp too; one that is not stays text.
  The 1904 system moves every date. A test reads a workbook written by `rust_xlsxwriter`
  (numbers, text with markup characters, a boolean, a date, a cached formula, a merged range)
  through the whole stack.

**Values that must not change** (fixes after review):

- A whole number beyond 2^53 (a 17-digit id) is kept as the digits the file has (`Cell::Integer`,
  a 64-bit integer) and never parsed to a double and stored altered. The rule: a column whose
  numbers are all whole is `int` (64-bit, exact); a column that mixes such a number with a number
  that has a fraction is **text**, never a rounded float; one that does not fit 64 bits is text.
- A cell arrives in any number of tokens (text, CDATA, comments between them), each small, so
  the **running length is capped at 128 KiB on every push**, not after the pieces are joined.
- CDATA is text, in a value and in an inline string, as in the shared strings (it was read as
  empty and became null).
- A boolean is `1` or `true` in any case (`TRUE`, `True`).
- A number format is elapsed time only for `[h]`, `[hh]`, `[m]`, `[mm]`, `[s]`, `[ss]`
  (any case); `[Magenta]0.00` or `[$-409]` is not. A value within half a second of midnight shows
  `23:59:59` of its own day instead of rolling over (in the 1900 system to the day that never
  existed).
- A batch that cannot be built is an error (`a batch could not be built`), never a batch dropped.

**Strings and names, after the second review.** A shared string is capped at 128 KiB
(`MAX_SHARED_STRING_BYTES`, a cell's cap) **as it is built**, on every push, so a string made of
many small tokens is cut as it grows; a workbook with one longer string is refused (`CellTooLong`,
an unreadable file), as an over-long inline cell is. A cell that points at a shared string checks
its length against the cell cap **before** it copies it into the cell. A whole number with a
leading `+` or a fraction of zeros (`12345678901234567.0`) is read exactly like the same number
without them. The zero-width non-joiner and joiner (U+200C, U+200D) are **not** stripped from
names: Persian, Indic scripts and emoji sequences need them. Still stripped: the bidirectional
controls, overrides and isolates, the other zero-width and invisible format characters, the
byte order mark, tag characters, U+2028 and U+2029, and the added ranges U+0890 to U+0891, U+110CD,
U+13430 to U+1343F and U+1BCA0 to U+1BCA3.

### Typing the columns (`xlsx_columns.rs`)

An xlsx cell already has a type, so a sheet's columns are typed **from the kinds of their
cells**, never by re-reading text. Two things follow. A number stays a number however many
digits it has (the CSV rules keep a value of more than 15 digits as text, which would turn
every computed column of a workbook, `0.1 + 0.2`, into text); and a text cell that looks like
a number stays text, as it is in Excel (a zip code `00501` or `01234` is never a number).

The rules are as conservative as the CSV's: a column whose cells disagree is text.

| The column's cells | Type |
|---|---|
| numbers, all whole and at most 2^53 in absolute value | `int` |
| numbers, any other finite one | `float` |
| booleans | `bool` |
| dates | `date`; dates with timestamps: `timestamp` |
| anything else: any mix, times of day, errors, numbers that are not finite, or no value at all | `string` |

A cell that contradicts the type of its column is a conflict (`Batcher::push` returns its
column) and the converter makes that column text and reads the sheet again. A text column
keeps every value as it would be shown: a number is its shortest exact decimal form (`42`,
`1.5`, `0.30000000000000004`), a boolean `TRUE` or `FALSE`, a date `YYYY-MM-DD` or
`YYYY-MM-DD HH:MM:SS`, a time `HH:MM:SS`. `Batcher` builds the Arrow batches of a sheet from
its rows and closes one at the CSV reader's bounds (8,192 rows, 1,000,000 cells or 8 MiB of
text), so a wide or text-heavy sheet gets shorter batches.

**A sheet with no header does not discard the workbook.** The header is the first row that has
a value. When a later row of the first 10,000 has a value past its last column (a title cell
above the table, say), that sheet has no names for its columns: it is **skipped, and the manifest
says so** in an optional `skipped` list (`sheet`, the cleaned name, and `reason: "header_row"`),
absent for a CSV. The other sheets are converted. If every sheet with a value is skipped the file
is refused (`unreadable_file`: a row has a value past the last column of the header). The limit of
the rule, exactly: it applies to what the sampling read sees, the first 10,000 data rows after the
header. A row **wider than the header that comes after those 10,000 rows is not skipped**: by then
parts of that sheet are already stored, so the sheet cannot be dropped cleanly, and **the whole
workbook fails** (`unreadable_file`: "a row has a value past the last column of the header",
which says what is wrong with that row). **Why `skipped` does not bump the manifest version.** `MANIFEST_VERSION` and the registry's
`FORMAT_VERSION` move together, and a bump would make every ready row (CSV ones too) claimable and
prepared again. `skipped` is optional and written only for a workbook with a skipped sheet: a CSV's
manifest is byte for byte what it was, and no host has prepared a workbook yet. A reader that does
not know the key and parses strictly (as this crate's own parser, with `deny_unknown_fields`, did
before this change) fails closed on exactly those manifests: it must treat the source as not prepared,
as for any manifest it cannot read. **Readers on the ADP side that need to accept it** (and the
optional `conversion` key, which already needed it): the preparation job binary (A3), the status
endpoint, the registry-side readers of `tables_json` (which does not carry it), and the prelude that
reads the manifest in the sandbox (C7). **The whole manifest is checked after every sheet**: the
table list against the 64 KiB registry row and then the whole file (tables, one conversion report
each, skipped sheets) against its 128 KiB cap, each with its own sentence, so 256 sheets fail at the
sheet that overflows and not late, as `internal`, at the manifest put.

**Names.** Table and column names have Unicode format characters (category Cf: bidirectional
overrides, zero-width characters, the byte order mark) replaced by `_` as control characters
are, and the manifest refuses them. Known limit, shared with the CSV and not changed: column names
are made unique case-sensitively (`a` and `A` are two columns) while table names ignore case.

**The table list of a workbook** (and the manifest, see above) is checked after every sheet against the 64 KiB registry row, so
many sheets fail early, with their own sentence (`table_too_large`: the table lists of all the
sheets do not fit the registry row; export fewer sheets or columns). A refusal before anything is
written (the byte cap) no longer asks the adapter to delete derived objects with an empty key
list, which an adapter could read as "delete by prefix". The spool's guard exists before the file
is unlinked, so a failed unlink cannot leak it.

### Opening the workbook and sampling a sheet (`xlsx_run.rs`)

`open_book` opens a workbook once (pre-check, sheets, shared strings, styles; see above) and
`Book` keeps it for the reads that follow. A sheet is read twice. `sample_sheet` is the first
read: the first row that holds a value is the header (so blank rows before it are dropped;
names are cleaned like a CSV's: control characters, length, empty, repeats), and the first
10,000 data rows decide the column types from the kinds of their cells (a later row is never
examined; the read stops there). It returns `None` for a sheet with no value, and a table with
no rows for one with only a header. A value past the last column of the header is refused
(`BeyondHeader`) and, like every failure here, the message carries no cell, sheet name or key.

The limits structures (`XlsxLimits`, `SheetLimits`, `ArchiveLimits`) are public types whose
fields are crate-private, so a host can only use the defaults and a test inside the crate can
lower a limit to move a boundary.

### Writing a sheet (`xlsx_run.rs`)

`run_sheet` is the second read of a sheet: it streams every row, typed, into the part writer.
Reading and typing run on a blocking thread and the writer on the async side, joined by a
channel of two batches, as for a CSV. The blocking half stops at the next row, or within 4,096 cells
whatever they hold, when its token is cancelled, or when the receiving side is gone, and a run that fails cancels it and waits for it,
so no thread outlives the run. A cell that contradicts its column's type comes back as
`RunEnd::Conflict` (column, and data row from zero), which the caller turns into a restart.

**Memory** is bounded by constants; the worst case is summed under *The memory test* below.

### Converting the workbook (`xlsx_convert.rs`)

`convert_xlsx` turns a workbook into one table per sheet that holds a value, in workbook
order, through the same `PartWriter`, `PartSink`, `ConvertControl` (every part key is recorded
**before** its put, so a dropped future leaves nothing untracked) and restart policy as a CSV.
The workbook is spooled (`XlsxSource::spool`), checked and opened once; then each sheet is
sampled and written (see above).

- A sheet with no value is not a table; a sheet with only a header is a table of no rows; a
  workbook with no table is refused (`NoData`). The table index is the position among the
  sheets that are tables, so the parts are `t0/…`, `t1/…`.
- A late cell that contradicts its column's type makes that column text and **only that
  sheet** is read again, at most three times, the third making every column text (which cannot
  conflict). `restarts`, `demoted` and `all_strings` are reported as for a CSV; the
  `ConvertedTable` of a sheet reports `utf-8`, no replacements and the blank rows it dropped
  (`blank_dropped`).
- The 50,000,000-cell cap is the **job's**: every cell any read counts (sampling, runs,
  restarts and skipped sheets), so a sheet that restarts three times counts four times.
- The table list of a sheet that cannot fit the registry row (64 KiB) is refused right after
  its first read, as for a CSV.
- Cancelling the control, or dropping the future, stops the read within 4,096 cells (looked at
  on every cell, so a sheet of empty cells ends too); the keys put so far are in
  `ConvertControl::paths()` for the caller to remove.

### Preparing a workbook (`driver.rs`)

`prepare_xlsx` is `prepare_csv` over a workbook: **the same driver, not a copy**. One private
`prepare` serves both, and only the conversion step differs (`convert_csv_table_with` for a
CSV, `convert_xlsx` for a workbook), so the claim, the lease (budget plus `JOB_GRACE`), the
tracking before every put (`track_blobs`, per table: the first part of a second sheet is
listed before it is put), the ownership checks, the 300 s budget, the bounded terminal steps,
the failure order (delete, then record) and the fixed failure sentences are one code path.

- A workbook larger than `PrepareEnv::xlsx_max_bytes` (default 400 MiB; 0 refuses every
  workbook) fails with `xlsx_too_large` **before a byte is read**, by the size the host
  declared in the request; the storage's own size and the bytes as they arrive are checked
  again while the workbook is spooled.
- The workbook is spooled to a local temporary file (see *Spooling the source*): progress
  reports the bytes spooled over the declared size, and the time budget covers the spool, the
  checks and the conversion together.
- The manifest holds one table per sheet that has a value, named by `unique_table_names` (clean,
  at most 64 characters, unique ignoring case: `Q3 Sales` and `q3 sales` become `Q3 Sales` and
  `q3 sales_2`) and one `conversion` entry per table. `PreparedTable::tables` replaces the
  single `converted`: a `Vec`, one per table, in manifest order.
- The sizes the row keeps (`prepared_bytes`, `blob_keys`) are the sums over every table and
  the manifest. A source that disappears while it is spooled releases the row, like a CSV.

### The memory test (`tests/xlsx_convert_memory.rs`)

Own test binary with a counting global allocator, in the style of `tabular_convert_memory`: it
measures the peak of live heap bytes while a generated workbook is converted, with the spool,
the zip reader, the XML parser, the shared-strings table, the batches and the part writer all
inside the measurement (the workbook file is written before it starts; the generator streams).
Parts are limited to 8 MiB, as in the CSV test.

| Workbook | Size | Peak |
|---|---|---|
| 100,000 rows of 8 short cells of every kind, 50,000 shared strings | 33 MiB | 23 MiB |
| the same with 400,000 rows | 127 MiB | 25 MiB |
| 300 rows of four 16 KiB text cells (a batch is held by its 8 MiB of text) | 18 MiB | 28 MiB |
| the same with 1,200 rows | 75 MiB | 28 MiB |
| 150,000 rows, deflated (the inflate path) | 9 MiB (about 50 MiB expanded) | 25 MiB |

(debug build, macOS; the numbers are what was measured, not the bounds.) **The bounds are not
tuned to those numbers**: a bound tuned to one machine's measurement already failed in CI once.
The allocator counts requested bytes, not time, so a slower runner changes only the
interleaving of the reading and writing halves, which can add at most the batches in flight
(one being built, two in the channel, one being written). Each scenario's absolute bound is
the worst case those add up to for its shape, with room above the measurement: 64 MiB for the
short-row shape and 112 MiB for the wide-text shape. The assertion that proves the memory is
bounded and not merely small is the other one in each scenario: the peak of a workbook four
times larger must be within a quarter plus 8 MiB of the smaller one's, which a sheet or a
shared-strings table held whole would fail by a wide margin.

**What the driver tests of workbooks pin** (`driver_xlsx_tests.rs`, with a storage that records
the registry's state at each put and each cleanup): every put of every table, the second table's
first part included, finds its key already listed in the row; a storage failure at table 1's
first part, after table 0 wrote three, deletes both tables' parts while the job still holds the
row and only then records `failed(storage)`; a row deleted as table 1's first part is put ends
the job as cancelled with the objects removed and no manifest put; a restart rewrites the same
keys and leaves none stale (a text run cannot have fewer parts than the typed run it replaces,
so a stale part cannot arise from a restart of a workbook).

**Worst case of a job, re-derived after the review fixes** (what each part can hold at its
limits; none of these sizes depends on the workbook):

| Part | Worst case |
|---|---|
| the spooled workbook (local file; counts here if the temporary directory is memory-backed) | 400 MiB |
| shared strings: text up to the 128 MiB declared, 10,000,000 `u32` offsets at most | 168 MiB |
| styles (65,536 styles, 65,536 custom formats) and the sheet list (256 names) | 4 MiB |
| the XML parser: one event (1 MiB), open elements (32 deep x 256 bytes), its 16 KiB buffer | 1 MiB |
| the zip directory kept: at most 10,000 entries | 1 MiB (the 8 MiB read to check it is freed) |
| one row: at most 16,384 cells and 1 MiB of text, one cell's pieces (128 KiB twice) | 2 MiB |
| batches alive at once: one being built (its records, then its columns), two in the channel, one written; each at most 8 MiB of text plus 8 MiB of numbers | 80 MiB |
| the part writer: an encoded row group and its output, at most 64 MiB each, and the slice being added | 136 MiB |
| arrow, the runtime and the compressor's own buffers (not measured) | 16 MiB |
| **sum** | **about 810 MiB** |

That is about 40 % of a 2 GiB job (the floor design D11 states for the preparation job; its
candidate is 4 GiB), with 409 MiB of it being the spool and the shared strings, which only a
workbook near the caps reaches. Measured heap peaks (above) are 23 to 45 MiB for 33 to 127 MiB
workbooks, because the worst cases do not coincide and the test data compresses. The first
version of this unit also held what a library's own directory discovery allocated (up to 65,535
entries with 64 KiB names) and what the parser kept for unclosed elements (gigabytes); both are
now bounded and have their own tests (`tests/xlsx_hostile_memory.rs`).

**Bounds in the memory test are derived, not fitted.** `grid_bound`, `wide_bound` and `part_cost`
are sums of named constants (`BATCH_TEXT`, `BATCHES_IN_FLIGHT_TEXT`, `STRINGS`, `FIXED`), and
`the_constants_the_bounds_come_from_are_the_production_ones` pins each to the production constant
(`BATCH_BYTES`, `BATCH_ROWS`, `BATCH_CELLS`, `PART_MAX_BYTES`, `PART_MAX_ROWS`, the token limit):
a batch twice as large fails that test, which a peak measured on compressible data could not
promise. The scenarios added after the review: production 64 MiB parts, a workbook of three
sheets (the peak is one sheet's), a sheet read again after a late conflict, and a sheet of 16,384
columns, refused after its sample (its table list cannot fit the registry row) at 4 MiB. Tests
that measure share global counters, so they take one lock and run one at a time.

**Last review fixes to the readers.**

- The manifest size is checked after every sheet, **skipped ones too** (a run of skipped sheets
  after the last table used to overflow the 128 KiB file only at the manifest put, as `internal`).
  The names of all the sheets with a value, tables and skipped, are made unique together (ignoring
  case), so a skipped sheet never shares a name with a table or with another skipped sheet;
  `Converted::table_names` carries the tables' final names and the driver uses them.
- The job's cancel token is given to the package **before its first read**, so the relationships,
  the workbook, the shared strings (up to 128 MiB) and the styles (up to 64 MiB) are inside the
  budget too.
- In the shared strings, an empty `si` inside a `si` (it would shift every later index), a `t`
  inside a `t` and a phonetic run inside a phonetic run (it would leak the rest of its text into
  the string) are refused.
- A sheet whose XML ends inside `row`, `c`, `sheetData` or `worksheet` is truncated and refused
  (`BadCell`), not accepted as complete.

**Names, again.** The name filter is no longer only a deny-list of format characters. `clean_name`
(shared with the CSV, for table, column and skipped-sheet names) turns control characters into `_`,
**removes** every character that shows nothing (the bidirectional controls and isolates, zero-width
and other format characters, the combining grapheme joiner, Hangul and Braille fillers, Khmer
inherent vowels, Mongolian and other variation selectors, tag characters, U+2028/2029), turns the
non-ASCII spaces (no-break, the en/em family, narrow no-break, medium mathematical, ideographic)
into a plain space, keeps the zero-width non-joiner and joiner **only between two visible
characters** (so a Persian or emoji name is unchanged), and trims. A name that is empty afterwards
(a header of only invisible characters, or only joiners) is named `columnN` or `sheetN` as an empty
one is, and two names that differ only by what was removed or normalised are the same name, so the
usual `_2`, `_3` suffix tells them apart. The manifest refuses a blank name and any name with a
character `clean_name` would have removed. For a CSV this also **trims** a header (`" name "`
used to keep its spaces).

**The size of what a job stores is capped.** A 128 KiB shared string referenced by 8 cells of each
row of a million rows decodes to about a terabyte of text from a small upload. Memory stays bounded
and the 300 s budget ends it, but only after hours of work have been paid for. The parts a
preparation stores are therefore capped in all at 1 GiB (`MAX_PREPARED_BYTES`,
`PrepareEnv::max_prepared_bytes`): the largest source the product accepts, so a legitimate source
never comes near it (a prepared copy is smaller than its source; the spike prepared a 1 GiB CSV to
297 MB). A part stored again by a restart counts once, as last stored; the manifest does not count.
Passing it fails with reason `output_too_large` ("the prepared tables are larger than the limit;
export fewer rows or columns"), the parts already stored are deleted and listed as for any failure.
Compressible text stores little (a repeated string compresses a thousand-fold), so for that input the
time budget remains the bound; the cap is what stops input that does not compress. It applies to a
CSV as well, through the same sink.

**No empty list to the adapter, anywhere.** A host adapter may read an empty key list in
`delete_derived` as "delete everything under the prefix", so no path sends one: `fail` (nothing
written), `source_gone`, `settle_lost` and `delete_best_effort` skip the call when they hold no key,
and the cleanup pass (`attachment_gc`) settles a row that tracks nothing without asking the adapter.
Each has a test with an adapter that records its calls, and each fails without the guard.

### Workbooks from real writers (`tests/xlsx_real_files.rs`, `tests/fixtures/xlsx/`)

Fixtures written by openpyxl and LibreOffice (not by this crate or `rust_xlsxwriter`), each with
an `.expected.json` that is the converter's output after `compare.py` checked it, cell by cell,
against what openpyxl reads back from the same file. `committed_fixtures_convert_to_the_values_recorded_with_them`
converts them all and compares; the ignored `dump_a_directory` (`XLSX_REAL_DIR`, `XLSX_REAL_OUT`)
does it for any directory of workbooks.

The first versions of this unit were tested with workbooks this crate or `rust_xlsxwriter`
wrote. The fixtures here come from other writers: **openpyxl 3.1.5** (`generate.py`, which also
post-processes a few files to carry what openpyxl cannot write) and **LibreOffice 7.4** (Debian
bookworm container, `soffice --headless --convert-to xlsx`), which also gives every formula a
cached value and writes shared strings (openpyxl here writes inline strings). `compare.py` reads
each workbook back with openpyxl and compares it with what the converter made, cell by cell; the
`.expected.json` beside each fixture is the converter's output after that comparison passed, and
`committed_fixtures_convert_to_the_values_recorded_with_them` keeps it so. The harness for any
directory of workbooks is the ignored `dump_a_directory` (`XLSX_REAL_DIR`, `XLSX_REAL_OUT`).

| File | Writer | What it has | Result |
|---|---|---|---|
| `multi.xlsx` | openpyxl | four sheets with data, a hidden one, an empty one, a title-row one, a chart sheet; strings with markup, accents, tabs; dates and datetimes; a boolean; a formula (no cached value); merged cells; frozen panes; an Excel table | tables `Sales`, `Lookup`, `Data`; `Report` skipped (`header_row`); the empty and chart sheets are no table; every cell equal; the formula cell null |
| `multi_lo.xlsx` | LibreOffice | the same, converted | same tables; the formula's cached value `37.7500001` read |
| `rich.xlsx`, `_lo` | both | rich text, an empty string, padded spaces | equal (rich runs joined; the empty string is null; spaces kept) |
| `dates1904.xlsx`, `_lo` | both | the 1904 date system, a timestamp with a second | equal (`timestamp` columns) |
| `wide_styled.xlsx`, `_lo` | both | 120 columns; 60 distinct number formats, fonts and fills; conditional formatting (cell rule, data bar) | equal |
| `ids_persian.xlsx`, `_lo` | both | a 17-digit id column, a Persian header with a zero-width non-joiner, mixed numbers and text | equal; the header keeps its joiner. Neither writer keeps the 17 digits (openpyxl writes `1.234567890123457e+16`, LibreOffice 15 significant digits): the converter reads what the file holds |
| `extlst.xlsx`, `_lo` | openpyxl + injected | an `extLst` as Excel writes it (x14 conditional formatting and sparklines, elements in other namespaces) | accepted, equal |
| `prefixed.xlsx`, `_lo` | openpyxl + rewritten | every worksheet element in the `x:` prefix | accepted, equal |
| `descriptor.xlsx` | Python `zipfile`, unseekable output | a data descriptor after each part (bit 3 set, local sizes zero), as Java and streaming writers produce | accepted, equal |
| `zip64_local.xlsx` | Python `zipfile`, `force_zip64` | zip64 extra fields in the local headers | accepted, equal |

No refusal and no mismatch was found, so no converter change came out of it. Not covered, for want of
the writer: files written by Excel itself (the container has none), and a zip64 end-of-central-directory
record (a streaming writer produces one only past 4 GiB or 65,535 entries, both over this reader's
limits, which refuses it as `Unsupported`).

## Running over prepared tables (`tabular_run`)

Module `tabular_run` takes a PREPARED file (the parts and manifest the driver wrote) into the Python sandbox and brings
results back. Dark like the rest: it is used only with `COLMENA_LARGE_TABULAR=on`, and with the switch off no code in it
runs. The sandbox side is described in [53_python_executors.md](./53_python_executors.md).

### Refusals (`refusal.rs`)

Every reason a run is refused is a `RunRefusal` with a stable `code` and a sentence for the model. There is no variant
that offers the original file: a refusal is never a fallback to loading it into memory. The sentences are fixed text. They
hold no storage key, no signed URL, no registry error detail and no cell; the only echo is a table name the model asked
for, cleaned as inert text and clipped to 64 characters. A failure reason is shown as one of a few fixed sentences chosen
from the reason code the preparation recorded (an unknown code reads as an internal error; the recorded detail is free
adapter text and is never shown).

| Code | Variants | The model can |
|---|---|---|
| `large_tabular_disabled` | `NotEnabled` | nothing: the environment does not support it |
| `large_tabular_not_ready` | `NotPrepared`, `StillPreparing` | retry shortly |
| `large_tabular_failed` | `PreparationFailed` (final or not), `BeingRemoved` | nothing, or wait for a new attempt |
| `large_tabular_over_budget` | `OverBudget` (data, part, volumes) | name fewer tables, or retry later |
| `large_tabular_no_such_table` | `NoSuchTable` | list the tables and choose one |
| `large_tabular_invalid` | `Invalid` (record, no root, manifest, parts) | nothing: the copy does not match its record |
| `large_tabular_storage` | `Storage` | retry later |
| `large_tabular_unavailable` | `Unavailable` (no staging root, mounts disabled, unsupported executor, registry, executor) | nothing, or retry later |

`to_tool_error()` is the object a tool returns: `{error, code, source: "execution"}` and nothing else.

### Verifying the prepared copy (`verify.rs`)

Before anything is staged the copy must be vouched for by the registry. The `source_key` comes from the session's own
catalog row, never from the model (the model names an attachment, not a key). `judge_row(row, source_key)` is the pure part:
no row is `NotPrepared`; `running` is `StillPreparing`; `deleting` is `BeingRemoved`; `failed` is `PreparationFailed` with
the recorded reason code (final once the attempts reach `MAX_ATTEMPTS`). A `ready` row must be this source's, in
`FORMAT_VERSION`, and carry a manifest key, else `Invalid(Record)`.

`PreparedTables` is the verified plan: the manifest and the part keys, no data. A part key is the storage's derived root
and the canonical relative path (`t<n>/part-NNNNN.parquet`, built only by `manifest::part_path`). `select(names)` chooses
tables by name (ignoring case, each once, in manifest order); a name that is not a table is `NoSuchTable`.
`DATA_MAX_BYTES` (1 GiB, `D_max`) is the most prepared bytes one call may stage; an estimate until the instance is measured.

`verify_prepared(registry, storage, source_key)` is the whole check, in order. The registry row is read (`get`) and judged
first; a registry that cannot be read is `Unavailable(Registry)` and its error text is dropped. Only then is the storage
asked: it must answer `derived_root(source_key)` (else `Invalid(NoRoot)`), and the row's manifest key must be exactly
`<root>/manifest.json` and be among the row's tracked blobs; a manifest key anywhere else is not the copy this source's
storage placed there. `prepared_bytes` (what the row recorded for the live parts and manifest) above `DATA_MAX_BYTES` is
`OverBudget(Data)`, decided from the row before any object is opened. The manifest is read under `MANIFEST_MAX_BYTES`
(128 KiB) however the storage declares its size: the declared size is checked first and the stream is cut off at the cap,
so an object that lies about its size is never buffered. A manifest that does not parse is `Invalid(Manifest)` and the
parse error is not shown. A verification opens only the manifest; no part is read.

Two cross-checks tie the storage to the registry. The manifest's table list must equal the row's `tables_json` byte for
byte (the row stores exactly `Manifest::tables_json`), else `Invalid(Manifest)`; and every part the manifest lists must be a
blob the row tracks, else `Invalid(Parts)`. A manifest the storage holds but the registry did not record is not trusted.

### Staging the parts (`stage.rs`)

`stage_tables(storage, plan, tables, data_dir, limits)` copies the manifest and the parts of the chosen tables into a
directory the trusted side owns (the call's `data` directory, which the jail binds read-only at `/data`; see
[53_python_executors.md](./53_python_executors.md)). `data_dir` must exist and be empty.

- *What is written.* `manifest.json` (the verified manifest serialised again: no byte of the stored file reaches the call
  unparsed) and `t<n>/part-NNNNN.parquet` for each part of each chosen table, the table index being the manifest's.
  Paths are built only from `manifest::part_path`. Directories are `0755` and files `0644` whatever the umask, so the slot
  user that reads through the bind can read them.
- *Streamed.* Each part is copied chunk by chunk to its file; the memory held is one chunk, whatever the part's size.
- *Bounded.* A part may be at most `PART_FILE_MAX_BYTES` (128 MiB, twice the size the converter rolls at) and the call at
  most `StageLimits::total_bytes` (`DATA_MAX_BYTES`, 1 GiB). Both are checked on the size the storage DECLARES, before the
  part's file exists, and again on the bytes that actually arrive.
- *Refusals.* Over a limit is `OverBudget(Part)` or `OverBudget(Data)`; bytes that differ from the declared size are
  `Invalid(Parts)`; a storage failure is `Storage`; a failure of the directory itself (a name already taken, a link, a full
  volume) is `Unavailable(Executor)`. No refusal carries an adapter's text or a path.
- *Nothing outside, nothing left.* Every file is created new, so a name that already exists (a link included) is refused and
  never followed. A refused or failed staging removes everything it wrote.

The proofs, in `stage.rs`: a part declaring over the part limit is refused with no chunk read; a stream that declares little
and never ends is cut off at the limit (at most one chunk past it); the call total is enforced on arriving bytes; bytes
that differ from the declared size are refused; a storage failure mid-part hides the adapter's text; staging 64 MiB in
1 MiB chunks never has more than two chunks alive (a guard counts the live bytes of each chunk); a link in the directory is
never followed. They run on any Unix (no jail is involved).

### The Python the model's code finds (`prelude.py`, `prelude.rs`)

The prelude is trusted code that runs before the model's code in the same `restricted` sandbox (the validator checks the
wrapped code, prelude included, so the prelude imports only what the allowlist allows: pandas reads the Parquet, pyarrow is
loaded by pandas and never imported). It gets its metadata from inputs the trusted side injects from the VERIFIED manifest
(`_ct_tables`, `_ct_data_dir`, `_ct_read_max`), never from a file the sandbox could have changed. `wrap_large_code(code)`
wraps the model's code like the small path (`pd`, `np`, `stats`, `result = None`, and the SAME postlude, taken from
`wrap_user_code` itself so the two cannot drift) with the prelude in place of `df = pd.DataFrame(...)`.

| Name | Behaviour |
|---|---|
| `tables.names` | the table names (sheet names, or one name for a CSV) |
| `tables.schema(name)` | `{name, rows, parts, columns: [{name, type}]}`; no sizes, no paths |
| `tables[name]` | a lazy handle; holds no data and is NOT a DataFrame; matched ignoring case when unambiguous |
| `t.name`, `t.rows`, `t.n_parts`, `t.columns`, `t.dtypes` | metadata |
| `t.head(n=5, columns=None)` | the first `n` rows (1 to 1000) of the first part |
| `t.read(columns, filters=None)` | `columns` is REQUIRED and must be known names in a list; the memory is estimated from the manifest and a read estimated over the limit raises the guidance error and reads nothing |
| `t.parts(columns=None, filters=None)` | a generator, one DataFrame per part (at most 500,000 rows); the only path with no size limit and the only one that may omit `columns` |
| `df` | not loaded: any use raises the guidance error |

Using a handle as a DataFrame (`groupby`, `[...]`, `len`, iteration, any other attribute) raises `LargeTableError` (a
`ValueError`) saying how to read the table. There is no method that loads a whole table.

The read estimate starts from the manifest's `in_memory_bytes` and applies the per-type multipliers of the table in
[Manifest](#manifest-manifestrs) (bool x16, date x2, a string adds 57 bytes of Python object per row), then doubles it for the
moment Arrow's table and the pandas frame exist together. `READ_MAX_BYTES` (1,536 MiB, half of the heavy run's 3,072 MiB) and
the factor of two are ESTIMATES: the design calibrates them against the spike's item 2 records, which were not re-run against
this estimate. Reads call `pd.read_parquet(path, columns=, filters=, use_threads=False, pre_buffer=False, memory_map=False)`.

The prelude's tests run it in `restricted` mode through the in-process helper. The ones that need real Parquet parts
(`parts` visits every row once, `read` with columns and filters, the wrapped code end to end) make the parts with pandas and
need python3 with pandas, pyarrow and scipy; without them each prints a skip line, so a developer machine without pandas
runs the rest (names, schema, handle misuse, `df`, limits, estimate, wrapper, inputs).

### One call, start to finish (`runtime.rs`)

`LargeTabularRuntime::run(LargeRunRequest)` is what the routing hands a large file to. In order: `ensure_prepared` (the
preparation wait, 240 s by default; a switch that is off is `NotEnabled`, a preparation that is running is `StillPreparing`
with its percent, a failure is `PreparationFailed` with the recorded reason and whether it is final), then
`verify_prepared`, then the choice of tables (`tables` empty means all), then `run_with_mounts` on the executor with the
wrapped code, mode `restricted`, the heavy deadline (`HEAVY_TIMEOUT_SECS`, 300 s, on the large path only) and an output
volume of `OUT_MIB`. The `source_key` is the catalog row's, never the model's.

The answer is `LargeRunOutput {stdout, result, tables}` or a `LargeRunError`: `Refused(RunRefusal)` before any code ran
(the executor is never asked in that case), `Python(text)` for the code's own failure, `Timeout`, or `Internal(text)` for an
executor failure. A child that ended without a result and a `MemoryError` both read as one fixed sentence telling the model
to select fewer columns or iterate parts (the limit is memory or CPU; the sentence says "probably").

### Routing a call to the large path (`large_route.rs`, `attachment_run_python/large.rs`)

`DagToolExecutor::with_large_tabular(runtime)` wires the runtime; `attachment_run_python` asks `large_target(attachment_id)`
before it reads anything. A call is routed only when a runtime is wired AND the attachment's row in the start-of-turn
catalog snapshot is a reference to an object the host owns (`origin = host_storage_ref`, which is how a large
`storage_key`-only entry is registered). Anything else takes the path it always took: with no runtime `large_target` returns
at its first line and looks at nothing, and the decision reads only the snapshot, so a small file's call gains no registry
lookup and no storage call even with the runtime wired. A host-owned file with no runtime keeps the refusal
`fetch_attachment_bytes` has always given.

The routed call answers `{stdout, result, duration_ms, tables, error?}` where the small path answers
`row_count` and `columns`; `tables` is `[{name, rows, columns: [{name, type}]}]` and no size, path or key. A refusal is the
typed error object `{error, code, source}` and nothing else. The tool's optional `tables` argument (names to make readable;
default all) is parsed but left out of the schema every call sees (`#[schemars(skip)]`), so the schema the model sees today
is unchanged; a switch that is on shows it with the tool text (a later slice).

### What the model is told, and the wiring (dark)

`HashMapNodeRegistry::set_large_tabular_runtime(runtime)` gives the `llm_call` node the runtime (a `OnceLock`, like the host
token port). The node hands it to the tool executor only while the large tabular switch is on; with the switch off, or
without a runtime, nothing is wired, no call is routed and the tool is exactly what it always was.

While a runtime is wired and the switch is on, `attachment_run_python` is offered with two additions
(`build_attachment_run_python_tool_definition_for_large_files`): the large-file text appended to its description (`df` is not
loaded; `tables.names`, `tables.schema(name)`, `t.read(columns=[...], filters=[...])`, `t.parts(columns=[...])`, `t.head()`;
a whole table cannot be loaded; runs may take up to 5 minutes; no charts) and the optional `tables` argument. The text is a
Rust constant beside the tool (as the refusal sentences are), not an entry of `text/tools/`: the registry's orphan check
wants one builder per entry and this text is only ever an appendix of one tool.

The node-level proofs run the real `llm_call` node against a scripted model (`nodes/llm/large_run_turn.rs`): a call over a
host-owned file reaches the runtime and the model sees the tables and the result, with no key and no byte of the original
read; with the switch off the file keeps its usual refusal and the tool text is the usual one; with no runtime wired
the same; a runtime whose own switch is off answers with the typed refusal.

### What the host gives the engine

`PrepareConfig` gains `registry: Option<Arc<dyn PreparationRegistry>>`, the preparation registry the host keeps (the same
trait the driver and the cleanup use). When the engine assembles its node registry (`node_registry_from_config`),
`large_runtime(prepare, storage, python_exec::mounted_executor())` builds the runtime and hands it to the node registry ONLY
with the switch on AND a registry AND an executor with run mounts. Each missing piece leaves large files registrable but not
analysable: nothing is half wired, one warning is logged at start (whether a registry was given; never a key), the tool keeps
its usual description, and a call over a host-owned file keeps the refusal it has always had. `PrepareConfig::default()` has no
registry, so a host that does not opt in changes nothing.

### Reading back `/out` (`collect.rs`)

Everything in the output volume was written by untrusted code, which also chose every name. `collect_out(out_dir, limits)` is
the one place the trusted side reads it, and it follows the six requirements the run-mounts review note lists:

1. *Read before the unmount, after the child is dead.* The caller (`local`) reads while the call's `StagedCall` is alive; the
   executor has sent SIGKILL to every process of the call's uid before `run_staged` returns. Whether each of them has finished
   its last system call is not awaited (the executor offers no such confirmation); the reader does not depend on it: it holds
   file descriptors, so a name swapped afterwards changes nothing, and what it keeps is a checked size, not a promise about
   the content.
2. *One directory descriptor, `openat` with `O_NOFOLLOW | O_NONBLOCK | O_NOCTTY | O_CLOEXEC`, never a path.* The directory
   itself is opened with `O_DIRECTORY | O_NOFOLLOW`; names come from `fdopendir` on a copy of that descriptor and each entry is
   opened relative to it. A link fails the open (`ELOOP`), a pipe opens without blocking and is dropped after the next step.
3. *`fstat` every entry.* Only a regular file with ONE link is kept. A link, pipe, socket, device or directory is
   `NotARegularFile` (a directory is never entered: there is no recursion); a file with two names is `HardLinked` under every name.
4. *Caps.* At most `OUT_MAX_ENTRIES` (64) directory entries are looked at: a volume with more keeps NOTHING (the walk stops at
   65). At most `OUT_MAX_FILES` (8) files are kept, each at most `OUT_FILE_MAX_BYTES` (64 MiB) and all together at most
   `OUT_TOTAL_MAX_BYTES` (128 MiB), by LOGICAL size (`st_size`): a sparse file allocates nothing and still returns that many
   bytes, so the allocated size is never used. These two sizes are estimates until the instance is measured (spike item 5).
5. *Names are bytes.* A kept name is 1 to 64 bytes of ASCII letters, digits, `.`, `_`, `-`, not starting with `.` or `-`,
   ending in `.csv` or `.parquet` (lower case: the extension is the type limit). A name that fails is `BadName` and is NEVER
   echoed; a name that passed may be shown with the reason it was not kept (`TooLarge`, `OverFileCount`, `OverTotal`...).
6. *The budget is reserved before the volume exists:* `SubprocessExecutor::stage_call` (see
   [53_python_executors.md](./53_python_executors.md)).

Nothing is executed and nothing is read here: the result holds open, checked descriptors (`OutFile`) which the caller streams
within the call's lifetime. The tests make every hostile case on a real directory (links to files, directories and nothing,
pipes, directories, hard links, a 2 GiB sparse file, names outside the charset including bytes that are not UTF-8, counts and
sizes at and past each limit, a name swapped after the check) and on the real jail in `tests/tabular_run_mounts.rs`.

A trigger error from `ensure_prepared` is terminal: the host's trigger answers it for a file it will never prepare (the ADP
side defines this), so it is `NeverPrepared` (`large_tabular_failed`, "cannot be prepared ... will not be retried"), not a
retry-later and not an executor problem; the adapter's text is dropped.

### Handing the outputs on (`OutputSink`, `outputs.rs`)

`MountedCall.sink` is where the kept outputs go. The subprocess executor reads `/out` with `collect_out` after `run_staged`
returns and BEFORE the staged call is dropped (the volume is unmounted only then), and calls `sink.accept(OutFile)` for each
file the reader kept; `MountedResult` lists `emitted` names, `rejected` entries (a name only when it passed the charset) and
`too_many_entries`. With no sink whatever the code wrote is discarded. `StoreSink` streams each file to the host's storage with
`store_stream` (`Generated` placement) one 1 MiB chunk at a time, so no output is held whole; the size the reader checked must
arrive, and a file truncated after the check is refused and stored nowhere. Not built: registering the stored outputs as
attachments in the registry, and the Python `emit_table` helper that writes them; the runtime does not yet pass a sink.
The jail proof (`hostile_outputs_never_reach_the_sink_and_the_good_one_does`) lets real code leave a good file, a name outside
the charset, a hard link, and a link and a pipe where the sandbox allows making them.

### The `/v2/run` wire (`wire.rs`)

One request and one response, each a stream, framed by us so the same bytes travel over HTTP/1.1 chunked transfer and over
HTTP/2 (nothing depends on a streaming feature of either). A frame is a big-endian `u32` length (at most 64 KiB) and that many
bytes of JSON.

| Part | Content |
|---|---|
| Request | `frame(CallHeader {v:2, code, mode, timeout_ms, inputs, out_mb})`; for each file `frame(FileEntry {path, size})` then exactly `size` raw bytes; then an empty frame. Nothing may follow it. |
| Request paths | only `manifest.json` and canonical `t<n>/part-NNNNN.parquet`; never a key, URL or host path |
| Response (200) | `frame(ResponseHeader {v:2, status: ok\|python_error\|timeout\|internal, message, output, stdout, files:[{name,size}], dropped:[{name?,reason}], too_many_entries})` then the raw bytes of each listed file, in order |
| Refusal (non-200) | JSON body `{refusal: busy\|mounts_disabled\|no_staging_root\|too_large\|bad_request, reason?}` |

Sizes are declared per file when the file starts, so a sender never needs the size of everything before its first byte. A
receiver checks each file's bytes against its declaration and the running total against its cap. `Reader` holds at most one
chunk and reads the next only after the previous is consumed, so the transport's backpressure holds the sender; every wait has
an idle limit (30 s without a chunk) and the whole transfer a deadline (240 s); the header must arrive within 10 s. Errors carry
no peer text.

### The server side (`serve.rs`)

`POST /v2/run` exists only on a `serve` that has a staging root (which is read only with `COLMENA_LARGE_TABULAR` on), behind the
SAME gate as `/v1/run`: the bearer token compared by digest, readiness, and the in-flight bound (no new scheme). In order:

1. The executor is asked `mounts_unavailable()` BEFORE a byte of the body is read: no staging root is 501, a template that
   turned the mounts off is 503 `mounts_disabled` with its reason (letters, digits, `_`).
2. The header frame must arrive within 10 s and be valid (version 2, mode `none` or `restricted`, output size 1 to 1,024 MiB).
3. The volume is taken with `SubprocessExecutor::stage_call` (the budgeted path): over the budget is 503 `busy` with
   `Retry-After`.
4. Each file is written into the call's data directory as it arrives, at most one chunk in memory. Only `manifest.json` and
   canonical `t<n>/part-NNNNN.parquet` paths are created, each once, with `create_new`; a file over its cap (128 MiB, the
   manifest 128 KiB) or past the call total (1 GiB) is 413 `too_large` BEFORE its bytes are read; bytes that differ from the
   declaration, bytes after the end, a stall (30 s idle) or a transfer over 240 s are 400/408.
5. The code runs with mounts (`run_staged`); the code's own failure, a timeout and an executor failure are statuses of a 200
   response (the executor's text is replaced by a fixed one), a mounts-disabled refusal discovered at run time is 503.
6. `/out` is read through `collect_out` and the kept files are streamed from their open descriptors by a task that owns the
   volume, over a channel of two chunks (backpressure), with the same idle and total limits. The volume is released when the
   last byte is sent, when the client is gone, or when a limit passes. A client that disconnects mid-upload drops the handler,
   which drops the volume (off the async worker).

Logs carry the request id, the outcome, counts and durations: never code, inputs, outputs, paths or headers.
The proofs are in `tests/tabular_run_remote.rs` (Linux, root, `COLMENA_PYEXEC_JAIL_TESTS=1`, one test at a time; the stall
test takes the 30 s idle limit).

### The client side (`remote_seam.rs`)

The request body is produced while the parts are read from storage: one part at a time, one chunk in memory, through a channel
of two chunks, so the HTTP client's backpressure holds the storage read; each file declares its size when it starts and the
bytes counted from storage must equal it (a storage that sends more or fewer aborts the body with an error, so the server never
takes a short file for the real one); a part or the call over its limit is refused before its bytes are read. Only canonical
paths go out, never a storage key or URL. The response header is waited for as long as the code may run plus the idle limit,
then each kept output is spooled to an anonymous private temporary file (bounded by the collector's caps) and handed to the
sink; its name and size are checked again here. A server that sends more files or bytes than the collector would keep is not
followed. Dropping the call drops the request, which makes the server release the volume. Nothing of the executor's or the
server's text reaches the model.

### To measure on the deployed service (spike item 3), and what was proved locally

Proved here, on loopback with HTTP/1.1 (the in-process router and the real jail): the server and client of this section, with
backpressure, size checks, stalls, cancellation and budgets (`tests/tabular_run_remote.rs`). The framing is ours and does not
use HTTP/2 features, so it should carry over HTTP/1.1 chunked transfer as well as h2c. NOT verified, and to be measured on the
deployed executor and its front end before the switch is enabled in dev:

- the largest request body the front end accepts on a streamed (chunked or h2) request without a content length (up to ~1 GiB of
  parts here), and the largest response;
- the longest a single request may last (the upload of up to 240 s, plus the run, up to 300 s, plus the download of up to 240 s;
  the client's whole-call bound is their sum) and whether any idle limit in front cuts a quiet request during the run;
- whether the front end BUFFERS the request body before forwarding it (which would defeat the one-chunk bound and add latency and
  memory there) or the response;
- whether the response can start only after the request body is complete (a front end that does not do full-duplex), which
  this protocol does not need: the server answers only after the whole upload;
- how many concurrent streams and connections one instance serves while the volumes budget (2 volumes in flight) is full, and
  what a client sees when it is refused at the front instead of by the server (a bare 503/429 is read as busy only with
  `Retry-After`);
- sustained throughput of a 1 GiB upload (the idle and total limits, 30 s and 240 s, are estimates);
- on HTTP/2, that a server that answers early (a refusal) while the body is unread resets the stream in a way the client reads
  as a refusal and not as a dropped connection (locally on HTTP/1.1 a refusal before the body is read can look like a dropped
  connection; the client therefore checks credentials and readiness first, with `warm`).

### Returning files from the code (`emit_table`)

`emit_table(df_or_parts, name, format='csv')` in the prelude writes `/out/<name>.<csv|parquet>`. A name is 1 to 48 letters, digits,
`_` or `-`; a format other than `csv` or `parquet`, a bad or duplicate name and a ninth file are refused in the sandbox
(`LargeTableError`) before anything is written, before pandas is even imported. CSV takes a DataFrame or an iterable of them
(one header, chunks appended: `t.parts(...)` streams); parquet takes ONE DataFrame (pyarrow cannot be imported to append). The
limits it enforces are the collector's (8 files, 64 MiB each, 128 MiB together, passed in as inputs from `CollectLimits`): a CSV
is measured after each chunk and a parquet after the write, with a read-only memory map (no `open()` is available), and a file
over a limit raises an error saying to return fewer rows or aggregate. That check is advisory: the reader enforces the same
limits again and drops what is over. The helper records `{name, format, rows, dtypes, size}` of each file; when any file was
written the wrapped answer is `{"__colmena_emitted": [...], "result": <result>}` and `unwrap_emitted` splits it, cleaning every
field (it comes from the sandbox: it is only shown beside a file the reader kept, matched by name). The tool text names it.

### The runtime stores the outputs

`LargeTabularRuntime::run` now passes a `StoreSink` (session ids from the request, `Generated` placement) as the call's sink, for the
local and the remote executor alike: the sink takes an `OutFile`, whichever executor produced it. `LargeRunOutput.emitted`
lists what reached storage (`name`, `mime_type`, `size_bytes`, the engine's `storage_key`), each described with the rows and
dtypes the code reported for it when the report matches a kept file by name (untrusted, cleaned; a report for a file that was
not kept is ignored), and `not_kept` says in words what was written and dropped (a name only when it passed the charset). The
result is the code's own (`unwrap_emitted` removes the report).

### Returned files become attachments

The routed answer gains `emitted` (`name`, `mime_type`, `size_bytes`, `document_id`, `rows`, `dtypes`) and `not_kept` when there are any.
Each returned file is registered with `DagToolExecutor::register_stored_attachment`, the path every generated file takes
(`register_attachment_bytes` now calls it too): provider `Generated`, the engine's own key as the id, origin
`generated_by:attachment_run_python`, never a host reference, so later tools can use it. Registration is fail-soft like the small
path's: a registry that refuses is logged and the file stays in storage. The `document_id` is the engine's own handle for a generated
object, as it is for every generated attachment; no host key or URL is in the answer.

### Progress, the call's total clock, and what the design's other guards decide

A routed call runs under `with_progress_ticker`: a `tool-progress` event every `TOOL_PROGRESS_INTERVAL_SECS` (10 s, stage `running`) under
the model's tool call id, and the call is bounded at 900 s in total (the ticker's longest); past it the step is dropped (which
closes the remote request, kills the local child and gives the volume back through their drop guards) and the answer says
the call did not finish. Required for correctness, not a nicety: the run loop's idle watchdog cuts a stream that is silent for 300 s
and a heavy call can be (preparation wait 240 s, run 300 s, transfers). The stage is `running` for the whole call; separate
`preparing`/`staging`/`collecting` stages would need the runtime to report its phases and are not built. That the id matches the
one the client finds the row by (`tool-input-available`) was not checked against a live stream.

| Design item (D7) | Decision | Why |
|---|---|---|
| progress ticker | built, required | without events the idle watchdog (300 s) can cut a heavy call |
| session lock (`heavy_run_locks`, 720 s lease) | safe to defer | one heavy run per session is a fairness and abuse control, not a correctness one: the executor's budget of volumes (2) already bounds heavy runs per instance with a typed `busy` refusal; the lock needs a new table in both dialects and a host decision |
| source-removed cancel (registry row within 10 s) | safe to defer | a call that already has its data works on its own staged copy and ends within its bounds; a source deleted before the data is read fails the read with a typed storage refusal; the cost is at most a few minutes of compute |
| `last_used_at` touch at staging | already done | `ensure_prepared` records the use |

A request header may carry `probe: true`: the server answers `204` after the checks that need no data (the route, the gate, mounts,
a free volume, a valid header) and runs nothing. The client sends one first, so what the server would refuse is known before
storage is read or a byte uploaded: on HTTP/1.1 a server that refuses while the body is unsent can close the connection, and a
client then sees a dropped connection, not the refusal. A volume taken between the probe and the call is still refused, then
reported as "unavailable, retry later" (a refusal read from the early answer when the connection survives).

### Staging: what the review changed

- A file the call will read is finished with flush AND sync, every error reported (the manifest used to be left to its drop, so a failed
  write could leave a truncated `/data/manifest.json`).
- The 1 GiB data limit is applied at staging, to the tables chosen and on the sizes their parts declare, not to the whole prepared copy
  at verification: naming fewer tables can now work. When the ONE table asked for is over the limit the refusal says so (`Budget::Table`,
  "this table alone is larger than ...") instead of asking for fewer tables.
- A part that sends more than it declared is a disagreement with the record (`Invalid(Parts)`, found at the first byte over); one that ends
  cleanly but short is a storage that cut the transfer (`Storage`, retryable).
- Every wait on storage is bounded: an idle limit per chunk (30 s) and a deadline for the whole staging (240 s), in the local stager and
  in the remote client's body producer. A storage that stalls or trickles holds no volume past them.
- The manifest's table list must serialise AND equal the recorded one; a row without a list never matches.

### Answers: retry, fixed text, bounded result, one wording

- *Retry or not.* Every refusal has `retryable` (an exhaustive `match`, so a new one cannot be left unclassified) and the tool error carries
  it, as do the timeout and executor-failure answers. Retryable: not prepared yet, still preparing, a non-final preparation failure, the
  executor's volumes full, a storage failure (including a stream cut short), the registry or the executor unavailable. Not retryable: switch
  off, a file that will never be prepared, being removed, no such table, any over-budget limit (a different request is needed), any
  `Invalid` (the record and the storage disagree; a missing part reads the same record again), no staging root, mounts disabled, an executor
  that cannot do it, a misconfigured one (`Misconfigured`, e.g. an output size the executor refuses).
- *Executor failures reach the model as fixed text* (`LargeRunError::Internal` carries nothing); the detail is logged, because it can name
  the host's socket path under `/run/...`.
- *The result is capped* like stdout (50 KiB of its JSON); past it the model gets a cut and a `result_note` saying to aggregate or use
  `emit_table`.
- *`tables` is read leniently, and only on the large path,* from the raw arguments (null, a string, odd members never fail the call); the
  args struct has no such field, so a wrong type cannot fail parsing on any path. `rows`/`dtypes` of returned files are named
  `rows_reported_by_code`/`dtypes_reported_by_code`: the code wrote the file and the report.
- *One source of truth for the wording:* `refusal_text_for(served)` where `served` is "the switch is on and a runtime is wired for this
  turn" (the load_attachment resolver, the whole-object fetch). The global constant stays `false`. The tool is offered with the large-file
  text and the `tables` argument only on a turn whose catalog holds a host-owned row. Not changed: the `$attachment:` placeholder resolver
  of `http_request` still uses the constant.

A directory listing that fails part-way fails the collection (`readdir` returns null at the end and on an error; errno tells them apart),
so entries cannot go missing silently on an I/O error.

The volume of a call is held by a guard (`Volume`) that hands the release (an unmount and a removal of up to a gibibyte) to the blocking
pool on every path: a normal end, an error, a panicking sink, the call's future dropped; the budget share goes back with it. The server
uses the same guard. Proved on the real jail with a sink that sees the volume still held while it runs, fails, panics, and hangs until the
call is cut.

### Outputs are transactional, and there is a per-conversation quota

- *Nothing is reported as kept unless it is stored AND registered.* The runtime returns the stored files with an `OutputGuard`; the holder
  commits it after registering them, and a guard dropped without it deletes the objects. A sink dropped with outputs it never handed out
  deletes them too. That covers a later `accept` failing, the call's 900 s clock dropping the future mid-collect, the tool future dropped
  before registration, and a registration that fails (a missing registry or session counts: the rows already made are taken back and the
  answer says none was kept). The deletes run on the runtime and a failed delete is logged. The remote client now hands outputs to the sink
  only after the whole response ended correctly, and rejects a repeated name in any case.
- *Quota:* at most 40 files and 512 MiB returned by this tool in a conversation (`SESSION_MAX_FILES`, `SESSION_MAX_BYTES`, estimates), counted
  from the registry's rows with this tool's origin. At the quota a call is refused before it runs (`large_tabular_quota`, not retryable); a
  call that would cross it keeps none of its files, says so, and still returns its result.

The remote client spools every output of a response and validates the whole of it (names, a repeated name in any case, sizes, the file
count, nothing after the end) BEFORE the sink sees the first; a hostile or broken server gets nothing into the sink.

### Wire limits, rates and hostile peers (review round)

- *Frames:* the frame cap is 1 MiB (`HEADER_MAX`), checked on the SENDING side (`try_frame`, no unchecked `as u32`). The client checks the call
  header before anything else: code and inputs that do not fit are a model-readable error ("shorten the code"), never "unavailable". The server
  cuts the response header to fit (stdout 64 KiB, the result's JSON 512 KiB, with a marker), so a completed run is never reported as a failure
  to answer.
- *Rates:* an upload and a download must keep 256 KiB/s once a 15 s grace has passed (`Reader::with_min_rate`, and the same rule on the
  server's sender). A peer that trickles bytes under the 30 s idle limit no longer holds a volume for the whole 240 s. Remaining exposure,
  stated: a token holder can still hold a volume for 15 s plus as long as it keeps above 256 KiB/s up to the 240 s limit, and with two volumes
  in the budget two such calls fill it; the answer to the rest is the authentication, not a larger rule.
- *The probe mounts nothing:* it is answered from the budget alone (`volumes < 2` and room for `out_mb`), so it can neither take a share a
  real call needs nor leave one to be released later.
- *Distinct answers:* a template that is not ready is `executor_not_ready` (retryable), not `mounts_disabled`; a staging volume whose I/O fails
  is `volume_io`; the template's reason is cleaned with the same charset as the other path; `collect_out` runs on the blocking pool.
- *The client bounds every wait:* a refusal body (the probe's too) is read to 16 KiB within the idle limit; the whole call has an outer limit
  (probe + upload + run + download + idle); a redirect is not followed.
- *Tests without privileges* (they run in ordinary CI): a hostile-server suite against the real client (endless refusal bodies on the probe and
  on the call, a redirect, an oversized response frame, code too large for a frame, plus the spooled-response cases), and the upload limits
  of the server's `receive` (total and part caps on the declaration, `MAX_FILES_IN`, a table index out of range, an oversized frame) and its
  header clamps. Not possible without privileges: building the router, because `AppState` holds the concrete `SubprocessExecutor`, which
  needs root; the 401 proof on the probe (no template start, no volume) stays in the privileged suite.

### Phase budgets, a stalled storage, and a copy that changes

- *Phase budgets add up under the call's clock:* preparation 180 s, staging the data 150 s, the code 300 s, reading back and storing the
  outputs 150 s (780 s) under the 900 s ticker bound, the rest being slack for the waits between them. The cut-off answer says where the
  call was: "while preparing the file", or "while staging the data, running the code or reading back its files" (the executor does not
  report finer phases to the runtime).
- *Every storage wait is bounded:* the manifest read (idle limit per chunk, 30 s for the whole read), each part (idle per chunk, the stage
  deadline), each store of an output (60 s, and 150 s for all of a call's). A storage that stalls or trickles ends the call with a retryable
  `Storage` refusal instead of holding the volume.
- *A copy that changes while the call runs is not answered.* The verified plan carries the row's generation (manifest key, layout version,
  attempts, last write, recorded size); after the run the registry is read again and a row that is gone, no longer ready or of another
  generation makes the answer `CopyChanged` (retryable) and the files the code returned are deleted. Decision stated: the check cannot sit
  between staging and the run (both happen inside the executor), so a re-preparation DURING staging can still let the code read a mix of
  generations, and then its answer is not given; the mix is never answered. A registry that cannot be read says nothing either way.
  `Invalid::Parts` no longer claims to compare the bytes read with the row: the row records no part sizes.

### Prelude reads, corrected (review round)

- Reads go through pyarrow reached by pandas' own optional-dependency helper (no import statement names it; the jail, not the validator, is the
  boundary, and the prelude's namespace already exposes pandas file readers) so they can decode with `date_as_object=False` (a date is
  `datetime64[ns]`, 8 bytes, not a Python object) and nullable booleans (`boolean`, a byte and a mask byte). The estimate's factors match
  what the frame is: bool x16, date x2.
- `head(n)` decodes ONE batch of `n` rows (`ParquetFile.iter_batches`), not the whole part of up to 500,000 rows; `parts()` with no columns
  is estimated per part and refused with the way out when one part is too wide to load.
- `emit_table` names are matched with `fullmatch` (a trailing newline no longer passes); a size that cannot be read (the file is missing, or
  empty after a write) raises an error and the file is not returned, instead of reading as 0 bytes under every limit; `in` follows the same
  case rule as `tables[name]`.

### Proofs that cannot pass silently

- The prelude tests that need python3 with pandas, pyarrow and scipy, and the jail test that makes a Parquet part, FAIL instead of skipping
  when `COLMENA_TABULAR_EXPECT_PANDAS=1` (set it in an environment that has the libraries, as the sandbox suite does with
  `COLMENA_PYEXEC_EXPECT_PYARROW`); without it they print a skip line. The privileged suites print the usual
  `skipped: set COLMENA_PYEXEC_JAIL_TESTS=1 ...` line, which the Linux job greps for; that job's step lists suites by name and these suites are
  not in it yet (a workflow change this work did not make): add `--test tabular_run_mounts --test tabular_run_remote --test tabular_large_e2e`.
- `tests/tabular_large_e2e.rs` runs the whole path once with nothing canned: the real tool routed to the runtime, a prepared copy verified
  through a real registry, real Parquet parts staged, the real prelude and wrapper in the real jail on the real subprocess executor,
  `/out` read back by the real collector, the file streamed by the real sink and registered. It asserts the result, the tables, the
  returned file's bytes in storage, its attachment row, no key in the answer, and the volume given back.

### Rates, spool, blocking work (last round)

- *The minimum rate is the peer's.* The clock starts at the peer's first byte and counts only the time spent waiting on it; the receiver's own time
  writing what it got, the executor's backpressure and a call's mounting are not counted (a late first byte is bounded by the idle limit and the
  phase budgets instead). The client's download rule is therefore plain 256 KiB/s after 15 s.
- *Header sizing:* the pre-flight check uses the real header (its `timeout_ms` included) and names what overflowed: the table list (name fewer
  tables) or the code (shorten it).
- *The response spool is bounded:* at most two responses spool at once (a counting limit), so a memory-backed temp directory holds at most
  2 x 128 MiB; the open and unlink run on the blocking pool.
- *Blocking work off the async workers:* `collect_out` runs on the blocking pool in the local executor as well as the server, and the server's
  "output ended early" send is bounded like the others.

### Names and dates (last round)

- *One rule for names that differ only in case, on every executor:* the collector keeps NONE of the colliding names and lists each in `not_kept`
  (`NameCollision`); the server and the client no longer disagree (the client's own rejection of a duplicate stays as a defence).
- *Table names:* `select` (the tool's `tables` argument) resolves a name like the prelude's lookup: exact first, else the single case-insensitive match;
  two variants and no exact match are ambiguous and refused.
- *Dates outside 1677-09-22..2262-04-11:* pandas 1.5 with `date_as_object=False` WRAPS such a date into a wrong one without an error (the test found
  `9999-12-31` coming back as 1816). Before converting, date columns' min and max are checked; out of range, that read keeps Python date and timestamp
  objects (heavier; the estimate does not know). `head(n)` accumulates batches until `n` rows exist.

### Classification, completed

- *A damaged copy:* a part the registry tracks that storage says it does not have (the adapters answer `InvalidInput` for a missing object) is
  `CopyDamaged` (`large_tabular_damaged`, NOT retryable as it is: "it will be prepared again"), in the local stager and the remote producer. The runtime
  then demotes the row with `mark_manifest_missing` (best effort, exactly the row observed), so the next claim prepares it again.
- *Server answers:* `executor_not_ready` and `volume_io` have their own typed refusals (both retryable); any 400 (wire version, bad `out_mb`, bad request)
  is `Misconfigured`, not retryable.
- *The code's timeout* is not retryable as it is (the same code repeats the same wait) and says to reduce the work (aggregate, read fewer columns,
  iterate `parts()`); *the call's 900 s cut-off* says which phase it was in and is not retryable as it is; an executor failure whose fixed text reads as a
  setup problem (not configured, no staging directory) is not retryable, any other is.

### One function decides the wording (`large_tabular::tool_served`)

`tool_served(switch_on, runtime_wired, tool_configured)` decides, for a turn, whether the large-file tool is served; the routing, the
`load_attachment` resolver and the whole-object fetch all use it, and the executor is wired with the runtime only when it is true, so a node that
does not offer `attachment_run_python` never points the model at it and never routes to it. `http_request`'s `$attachment:` resolver is built once
per process and cannot know a turn's tool list: it follows a shared flag set by the node registry (switch on AND a runtime wired), and so it can name
the tool in a turn whose node does not offer it. That residual is stated, not hidden.

### One cleanup owner for returned files (`OutputLedger`) and names unique to the call

- *Rows first, objects second.* The stored objects and the registry rows a call makes belong to one `OutputLedger`. Rolled back, or dropped, it removes the ROWS first and
  then the OBJECTS, each step bounded (30 s, a delete tried twice), and from `Drop` it runs spawned, on a small runtime of its own if none is current. A row that will
  not go keeps its object: the pair stays whole and is reported as kept, because it is.
- *Names unique to the call.* The port gives no key before a store (`store_stream` returns it), and what a host derives from `filename` is the host's contract; so every
  stored name is prefixed with an id unique to the call. A key derived from the name cannot repeat between calls, a later call's cleanup can never name an earlier call's
  object, and an object left by a store that was cut (whose key was never returned, so nothing can delete it) is logged by its unique name for the host's orphan sweep of
  generated objects. Local adapters key by uuid; the HTTP-callback adapter's keys come from the host. The attachment row's display name is still the file's own.

- *Quota.* A call at the quota still RUNS and answers (the result is returned); only keeping files is refused, all or none, with a sentence the model can act on
  ("return results in the answer"). Counting and registering happen under a per-conversation lock, so two concurrent calls cannot both pass within a process (across
  processes each instance can add at most one call's files, 8, past the figure). A usage that cannot be read fails closed for keeping files. The tool future dropped
  between two registrations, a second registration that fails and a row that will not come back are each tested (rows removed before objects is asserted).

## Demoting a damaged copy is guarded and bounded

A call that finds a part missing in storage answers `copy_damaged` (not retryable
as it is) and asks the registry to mark the copy missing so the next claim
prepares it again. Three rules keep that from hurting a healthy file:

- Only the generation the call verified is demoted. The plan keeps the verified
  row and the registry's conditional update refuses a row written since, so a
  reader of an old generation cannot flip a row prepared again meanwhile.
- Only a definite "not found" counts as missing. The storage adapters have no
  not-found variant; they answer `InvalidInput` with a "not found" message, and
  that message is the test. Other invalid input and every transient fault is
  the storage's moment (`storage`, retryable). A key missing from an adapter's
  in-memory index reads the same way; the cool-down below bounds that case.
- A source is demoted at most once per 15 minutes per process (at most 1024
  sources tracked). A storage that keeps answering "not found" cannot make every
  call prepare a large file again. The choice is a per-process cool-down rather
  than a counter in the row because a completed preparation resets the row's
  attempts; it is not shared between processes.

## The output ledger is cancel-safe and treats unknown outcomes as unknown

The ledger that owns the objects and registry rows of a call's returned files:

- Marks a row "may exist" BEFORE the registry is asked to make it. An upsert that
  committed but errored, timed out or was cut off is undone like any other
  (removing a row that is not there is a no-op).
- Keeps every entry until its step is confirmed. A rollback whose future is
  dropped midway leaves the remaining entries to `Drop`, which finishes them.
- Re-reads the row when a delete's outcome is unknown (an error or a timeout).
  Only a row confirmed absent lets the object go; otherwise the pair is left whole
  and reported as kept.
- Runs outside the conversation's lock: the lock covers the usage read and the
  registrations only, each bounded (10 s; 15 s to get the lock), and the rollback
  after it.
- Runs inside the call's clock: `keep_outputs` is part of the future the progress
  ticker bounds, so a registry that stops answering is cut with the rest.

## Date and timestamp columns are decided once per table

pandas 1.5 holds dates as `datetime64[ns]` (1677-09-22 to 2262-04-11) and wraps
a value outside it. The prelude therefore decides, once per table and column,
whether the column can be read as `datetime64[ns]` in EVERY part:

- Only date columns and timestamp columns not already in nanoseconds are looked at
  (the manifest's type says which). The footer's row-group statistics decide;
  a part without them, or a nested column, is scanned (that column alone).
- A column that does not fit stays Python objects in every part, so its dtype does
  not change between parts; the other date columns of the read stay `datetime64`.
- The read estimate uses the object size for such a column (40 bytes a row for a
  date, 56 for a timestamp) and `tables.schema(name)` reports each date or
  timestamp column's `dtype` as read (`datetime64[ns]` or `object`).
- Nothing is caught while converting: a `MemoryError` is the call's.

The manifest only has flat column types today; nested date types in a part are
still checked (lists, structs and maps are flattened for the check).

## Wording and classification of the remaining refusals

- A conversation already at its limit of returned files is told so with the answer
  (the call still runs, and its files are not stored at all, so nothing is uploaded
  to be deleted). A call that only crosses the limit is told its files "would
  exceed" it, with the number kept so far.
- Rejected credentials (401/403) are a typed, permanent setup problem
  (`large_tabular_unavailable`, `retryable: false`), never "retry later".
- An upload cut on the way is answered by the server as `503` with
  `refusal: upload_interrupted` and is retryable; only a request that is
  malformed (`400`) is a setup problem.
- The refusal shared by nodes that cannot know which tools the calling node offers
  (the `http_request` `$attachment:` resolver) never names `attachment_run_python`.

## Which tool serves a large file

Two tools take the `tables` argument over a prepared large file:
`attachment_run_python` (deprecated, kept for old graphs) and `data_run_python`.
`LargeServed::decide` (domain `large_tabular.rs`) is the one place that decides which of
them serves a turn, EACH ON ITS OWN: the engine switch is on, the runtime is wired, and the
node OFFERS that tool (`synthetic_tool_offered`: the tool's own availability condition, and
not excluded with `!name`). The executor is wired with that set, and a call that names a
tool outside it never reaches the LARGE path, whichever caller made it. (The agent loop
already refuses a name the request did not offer: `agent_service::dispatch_call`.) When both
serve, the refusals name `data_run_python`.

**Known pre-existing limitation, not changed here:** `DagToolExecutor::execute` has no
"offered" check of its own. The resume path (`execute_with_resume_answer`, replaying the call
a previous run left SUSPENDED) reaches it without passing `dispatch_call`, and the executor's
SMALL path of `attachment_run_python` / `data_run_python` runs a tool by name if some other
caller asks it to. Neither Python tool ever suspends, and every call a model makes goes through
`dispatch_call`, so a model cannot use it; but a future caller of the executor would have to
check offering itself.

Every message that points the model at a tool uses that answer, so a refusal never
names a tool the node was not given: the load-whole-file refusals say
``use `data_run_python` with `tables` ``, ``use `attachment_run_python` with `tables` ``
or, with no tool, that large-file analysis is not available. The three sentences carry the
same `large_tabular_file` code. The `http_request` `$attachment:` resolver is shared by
every node and never names a tool.

## `data_run_python` over a large file

`data_run_python` is the Python tool agents really have, so the large path is reachable
through it. **When it is available** (all four, decided per turn and per node):

1. the engine switch `COLMENA_LARGE_TABULAR` is on and the host wired a runtime
   (storage, preparation registry and executor);
2. the node OFFERS `data_run_python`: it is declared in `tool_configurations`, or named in
   `enabled_tools` (directly, as `"*"`, or through the `gsheets` toolkit alias), and not
   excluded: `!data_run_python`, `!*` and `!gsheets` all exclude it, as they always did
   (`inputs.enabled_tools` replaces `config.enabled_tools`, never merged);
3. the turn carries a host-owned file (a catalog row that is a storage reference) that is large;
4. the call names that file in `bindings`.

With 1-3 the tool is shown with the large-file text and a `tables` argument; with any of 1-3
missing it is exactly the tool it always was (no text, no argument, nothing routed). With 1-3
and a call whose bindings name no large file, the call also runs exactly as before.

A call whose bindings name a large file runs over its prepared tables the way
`attachment_run_python` does (`tables`, `emit_table`, typed refusals, progress, the
per-conversation quota and the output ledger are the same code). What differs:

- the file is passed as ONE binding, `bindings: [{"var": "big", "attachment_id": "<id>"}]`;
  `var` is not used. Mixing it with other bindings, or passing `code_ref`, is refused
  with `large_tabular_file` and `retryable: false`; nothing runs.
- the code may leave its answer in `output` (the `data_run_python` habit) or `result`;
  `result` wins when both are set. The answer is the large path's own (`result`, `tables`,
  `emitted`, `not_kept`, `stdout`), not the small path's `output`.
- `output_tables`, `output_sheets` and `output_attachments` are not available; files go back
  with `emit_table`. The returned files are attachments tagged `generated_by:data_run_python`
  and count against that tool's per-conversation quota (together with any file its small
  `output_attachments` sink registered).
- a refusal that sends the model to a tool names `data_run_python` when the node has it.

`attachment_run_python` keeps working for old graphs on the same terms; it is not the tool
that is named when the node has both.
