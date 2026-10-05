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
| `deleting` | no |
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
prepared, waiting at most `wait` (polled every second):

| Situation | Behaviour |
|-----------|-----------|
| Switch off | `NotEnabled`; the registry, the trigger and the progress port are not touched |
| `ready` | `Ready(row)` |
| `running` with a live lease | attach and wait; never a second trigger |
| No row | request once, unless the progress port says the host already queued or started it (then attach) |
| Any other row (nobody holds it) | request once, then wait |
| Wait ends first | `StillPreparing { progress }`, never an error |

`EnsureOutcome` has no variant that offers the whole file: a file that is not
prepared is never loaded whole into the sandbox, and `EnsureOutcome::message()`
words the state for the model. The call never writes the registry: claiming is
the job's business. This is the core of the lifecycle; the handling of failed
rows and attempts, de-duplication of concurrent callers, the bound on every
awaited call and the recording of use follow in later slices of this chain.

## Tests

`cargo test --lib attachment_prepared` applies the SQLite migration to an
in-memory database and checks the columns, the primary key and the defaults.
The Postgres twin is `#[ignore]`d like the other Postgres repository tests and
needs `DATABASE_URL` (`DATABASE_URL=postgres://... cargo test --lib
attachment_prepared -- --ignored`).

`cargo test --lib tabular_prepare_ensure` covers `ensure_prepared` with fake
ports and a fake registry under paused time. `cargo test --test
large_tabular_switch` runs in its own process: the switch is off by default, is
read once, and the engine refuses to start with the switch on and an unwired
trigger.

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
