# Connection Pool Management — Post-Deploy Validation

Validation checklist for the Phase 1 rollout of `ColmenaEngine` + `PgPoolRegistry`
(spec: `docs/superpowers/specs/2026-04-20-connection-pool-management-design.md`).

## Staging

1. `gcloud run deploy` the worker to staging.
2. Watch logs for the `engine_started` event on boot. Confirm `pinned_pool_count=1`.
3. Submit 10 representative jobs. In the worker logs you should see **no**
   `pool_created` event between them — only `pool_evicted` events are worth
   noticing.
4. `curl https://<staging-worker>/debug/pools`. Expected:
   - `cached_pools == 1` (internal only) if no graph used a second DB.
   - `pinned_pools == 1`.
   - `evictions_total == 0` under normal traffic.
5. On Cloud SQL, check:
   ```sql
   SELECT application_name, count(*)
   FROM pg_stat_activity
   WHERE datname = current_database()
   GROUP BY application_name;
   ```
   Expected: count ≤ `COLMENA_POOL_MAX_CONN_PER_URL` (default 2) per worker instance.
6. Test suspend/resume: trigger a `suspend` node graph, then resume. Confirm the
   state persists and no new pool is opened.
7. Test with a graph referencing an external DB in `connection_url`: expect a
   second entry in `/debug/pools` after the first call; reuse on the second call.

## Production

1. Deploy off-peak. Watch the first 15 minutes of logs for `pool_evicted`
   events — any warn-level eviction during low traffic is a red flag.
2. Monitor Cloud SQL connection count for 1 hour. If it exceeds 50% of
   `max_connections` sustained, open an incident and consider rollback.
3. Rollback is a plain `gcloud run services update-traffic` to the prior revision.
   No schema or Redis changes to undo.
