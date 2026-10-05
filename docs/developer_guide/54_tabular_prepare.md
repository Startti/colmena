# 54. Tabular prepare (large CSV/Excel preparation)

Module `tabular_prepare` will hold the pieces that prepare a large tabular
attachment once, ahead of the questions asked about it. Everything in it is
**dark**: it is used only when the engine switch `COLMENA_LARGE_TABULAR=on`,
and nothing converts a file or calls into the module yet. With the switch off
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
  no converter wired (nothing converts yet) it only logs.
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
