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
This slice has the SQLite implementation (`sqlite_registry.rs`) with `claim` and
`get`; the Postgres implementation follows in its own slice.

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

## Tests

`cargo test --lib attachment_prepared` applies the SQLite migration to an
in-memory database and checks the columns, the primary key and the defaults.
The Postgres twin is `#[ignore]`d like the other Postgres repository tests and
needs `DATABASE_URL` (`DATABASE_URL=postgres://... cargo test --lib
attachment_prepared -- --ignored`).

`cargo test --lib tabular_prepare` runs the registry cases against a
file-backed SQLite database: a claim creates a `running` row, a live lease is
refused, an expired one is taken over, a dead job is retried only up to the
attempt cap, the lease boundary holds at millisecond resolution, and eight
concurrent claims have exactly one winner. The cases are written once as
functions generic over the trait (`registry_contract.rs`) so the Postgres slice
runs the same ones.
