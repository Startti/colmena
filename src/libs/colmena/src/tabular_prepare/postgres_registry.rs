//! Postgres implementation of [`PreparationRegistry`]. The statements are the
//! ones SQLite runs ([`super::registry`]); Postgres takes `$N` as written and
//! compares `TIMESTAMPTZ` values natively.

use super::registry::*;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use std::sync::Arc;

pub struct PostgresPreparationRegistry {
    pool: Arc<PgPool>,
}

impl PostgresPreparationRegistry {
    pub fn from_pool(pool: Arc<PgPool>) -> Self {
        Self { pool }
    }
}

impl PostgresPreparationRegistry {
    /// Blob keys the row tracks now (empty when there is no row).
    async fn tracked_keys(&self, source_key: &str) -> Result<Vec<String>, RegistryError> {
        Ok(self
            .get(source_key)
            .await?
            .map(|row| {
                // The manifest a claim over an older format left in place is
                // superseded by `complete`; keep it tracked for cleanup.
                let mut keys = row.blob_keys;
                keys.extend(row.manifest_key);
                keys
            })
            .unwrap_or_default())
    }
}

#[async_trait]
impl PreparationRegistry for PostgresPreparationRegistry {
    async fn claim(&self, req: ClaimRequest) -> Result<Option<Claim>, RegistryError> {
        let row = sqlx::query_scalar::<_, i32>(CLAIM_SQL)
            .bind(&req.source_key)
            .bind(req.format_version)
            .bind(req.source_bytes)
            .bind(&req.owner)
            .bind(req.now + req.lease)
            .bind(req.now)
            .bind(MAX_ATTEMPTS)
            .fetch_optional(&*self.pool)
            .await
            .map_err(|e| backend_err("claim", e))?;
        Ok(row.map(|attempts| Claim { attempts }))
    }

    async fn complete(
        &self,
        source_key: &str,
        owner: &str,
        info: ReadyInfo,
        now: DateTime<Utc>,
    ) -> Result<TerminalOutcome, RegistryError> {
        let tracked = self.tracked_keys(source_key).await?;
        let row = sqlx::query_scalar::<_, String>(COMPLETE_SQL)
            .bind(source_key)
            .bind(owner)
            .bind(&info.manifest_key)
            .bind(blob_keys_to_json(&merge_keys(&tracked, &info.blob_keys))?)
            .bind(&info.tables_json)
            .bind(info.prepared_bytes)
            .bind(now)
            .fetch_optional(&*self.pool)
            .await
            .map_err(|e| backend_err("complete", e))?;
        Ok(match row {
            Some(_) => TerminalOutcome::Written,
            None => TerminalOutcome::Cancelled,
        })
    }

    async fn fail_with_blobs(
        &self,
        source_key: &str,
        owner: &str,
        error_code: &str,
        error_detail: &str,
        blob_keys: &[String],
        now: DateTime<Utc>,
    ) -> Result<TerminalOutcome, RegistryError> {
        let tracked = self.tracked_keys(source_key).await?;
        let row = sqlx::query_scalar::<_, String>(FAIL_SQL)
            .bind(source_key)
            .bind(owner)
            .bind(error_code)
            .bind(error_detail)
            .bind(blob_keys_to_json(&merge_keys(&tracked, blob_keys))?)
            .bind(now)
            .fetch_optional(&*self.pool)
            .await
            .map_err(|e| backend_err("fail", e))?;
        Ok(match row {
            Some(_) => TerminalOutcome::Written,
            None => TerminalOutcome::Cancelled,
        })
    }

    async fn track_blobs(
        &self,
        source_key: &str,
        owner: &str,
        keys: &[String],
        now: DateTime<Utc>,
    ) -> Result<TerminalOutcome, RegistryError> {
        let tracked = self.tracked_keys(source_key).await?;
        let row = sqlx::query_scalar::<_, String>(TRACK_BLOBS_SQL)
            .bind(source_key)
            .bind(owner)
            .bind(blob_keys_to_json(&merge_keys(&tracked, keys))?)
            .bind(now)
            .fetch_optional(&*self.pool)
            .await
            .map_err(|e| backend_err("track_blobs", e))?;
        Ok(match row {
            Some(_) => TerminalOutcome::Written,
            None => TerminalOutcome::Cancelled,
        })
    }

    async fn still_owned(&self, source_key: &str, owner: &str) -> Result<bool, RegistryError> {
        let row = sqlx::query_scalar::<_, i32>(STILL_OWNED_SQL)
            .bind(source_key)
            .bind(owner)
            .fetch_optional(&*self.pool)
            .await
            .map_err(|e| backend_err("still_owned", e))?;
        Ok(row.is_some())
    }

    async fn release(&self, source_key: &str, owner: &str) -> Result<bool, RegistryError> {
        let done = sqlx::query(RELEASE_SQL)
            .bind(source_key)
            .bind(owner)
            .execute(&*self.pool)
            .await
            .map_err(|e| backend_err("release", e))?;
        Ok(done.rows_affected() > 0)
    }

    async fn delete(&self, source_key: &str) -> Result<bool, RegistryError> {
        let done = sqlx::query(DELETE_SQL)
            .bind(source_key)
            .execute(&*self.pool)
            .await
            .map_err(|e| backend_err("delete", e))?;
        Ok(done.rows_affected() > 0)
    }

    async fn touch_last_used(
        &self,
        source_key: &str,
        now: DateTime<Utc>,
        min_interval: chrono::Duration,
    ) -> Result<bool, RegistryError> {
        let row = sqlx::query_scalar::<_, String>(TOUCH_LAST_USED_SQL)
            .bind(source_key)
            .bind(now)
            .bind(now - min_interval)
            .fetch_optional(&*self.pool)
            .await
            .map_err(|e| backend_err("touch_last_used", e))?;
        Ok(row.is_some())
    }

    async fn begin_delete(
        &self,
        row: &PreparedRow,
        owner: &str,
        lease: chrono::Duration,
        now: DateTime<Utc>,
    ) -> Result<bool, RegistryError> {
        let done = sqlx::query_scalar::<_, String>(BEGIN_DELETE_SQL)
            .bind(&row.source_storage_key)
            .bind(owner)
            .bind(now + lease)
            .bind(now)
            .bind(row.status.as_str())
            .bind(row.updated_at)
            .bind(row.last_used_at)
            .bind(row.last_used_at.is_none())
            .fetch_optional(&*self.pool)
            .await
            .map_err(|e| backend_err("begin_delete", e))?;
        Ok(done.is_some())
    }

    async fn finish_delete(&self, source_key: &str, owner: &str) -> Result<bool, RegistryError> {
        let done = sqlx::query(FINISH_DELETE_SQL)
            .bind(source_key)
            .bind(owner)
            .execute(&*self.pool)
            .await
            .map_err(|e| backend_err("finish_delete", e))?;
        Ok(done.rows_affected() > 0)
    }

    async fn delete_if_unchanged(&self, row: &PreparedRow) -> Result<bool, RegistryError> {
        let done = sqlx::query(DELETE_IF_UNCHANGED_SQL)
            .bind(&row.source_storage_key)
            .bind(row.status.as_str())
            .bind(row.updated_at)
            .bind(&row.lease_owner)
            .bind(row.lease_owner.is_none())
            .execute(&*self.pool)
            .await
            .map_err(|e| backend_err("delete_if_unchanged", e))?;
        Ok(done.rows_affected() > 0)
    }

    async fn find_stale(
        &self,
        cutoff: DateTime<Utc>,
        now: DateTime<Utc>,
        after: Option<&str>,
        limit: u32,
    ) -> Result<Vec<PreparedRow>, RegistryError> {
        let rows = sqlx::query(FIND_STALE_SQL)
            .bind(cutoff)
            .bind(now)
            .bind(after.unwrap_or(""))
            .bind(i64::from(limit))
            .fetch_all(&*self.pool)
            .await
            .map_err(|e| backend_err("find_stale", e))?;
        rows.iter().map(|row| Ok(prepared_row_from!(row))).collect()
    }

    async fn list_ready_after(
        &self,
        after: Option<&str>,
        limit: u32,
    ) -> Result<Vec<PreparedRow>, RegistryError> {
        let rows = sqlx::query(LIST_READY_AFTER_SQL)
            .bind(after.unwrap_or(""))
            .bind(i64::from(limit))
            .fetch_all(&*self.pool)
            .await
            .map_err(|e| backend_err("list_ready_after", e))?;
        rows.iter().map(|row| Ok(prepared_row_from!(row))).collect()
    }

    async fn mark_manifest_missing(
        &self,
        row: &PreparedRow,
        now: DateTime<Utc>,
    ) -> Result<bool, RegistryError> {
        let Some(manifest) = row.manifest_key.as_deref() else {
            return Ok(false);
        };
        let done = sqlx::query_scalar::<_, String>(MARK_MANIFEST_MISSING_SQL)
            .bind(&row.source_storage_key)
            .bind(now)
            .bind(row.updated_at)
            .bind(manifest)
            .fetch_optional(&*self.pool)
            .await
            .map_err(|e| backend_err("mark_manifest_missing", e))?;
        Ok(done.is_some())
    }

    async fn get(&self, source_key: &str) -> Result<Option<PreparedRow>, RegistryError> {
        let row = sqlx::query(GET_SQL)
            .bind(source_key)
            .fetch_optional(&*self.pool)
            .await
            .map_err(|e| backend_err("get", e))?;
        row.map(|row| Ok(prepared_row_from!(&row))).transpose()
    }
}

/// The contract cases of `registry_contract` against a real Postgres. Ignored
/// by default like the other Postgres repository tests (`DATABASE_URL`
/// required); run with `cargo test --lib tabular_prepare -- --ignored`.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag_engine::infrastructure::pool_registry::{PgPoolRegistry, PoolConfig};
    use crate::tabular_prepare::registry_contract::*;

    async fn make_registry() -> PostgresPreparationRegistry {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL not set");
        let registry = Arc::new(PgPoolRegistry::new(PoolConfig::defaults()));
        let pool = registry.get_or_create(&url).await.unwrap();
        sqlx::migrate!("migrations/postgres")
            .set_ignore_missing(true)
            .run(&*pool)
            .await
            .unwrap();
        PostgresPreparationRegistry::from_pool(pool)
    }

    macro_rules! pg_case {
        ($name:ident, $case:ident) => {
            #[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
            #[tokio::test]
            async fn $name() {
                $case(&make_registry().await).await;
            }
        };
    }

    macro_rules! pg_driver_case {
        ($name:ident, $case:ident) => {
            #[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
            #[tokio::test]
            async fn $name() {
                crate::tabular_prepare::driver::cases::$case(Arc::new(make_registry().await)).await;
            }
        };
    }

    pg_driver_case!(
        tabular_prepare_pg_a_prepared_table_is_ready_with_the_manifest_stored_last,
        a_prepared_table_is_ready_with_the_manifest_stored_last
    );
    pg_driver_case!(
        tabular_prepare_pg_without_a_derived_root_nothing_is_written_not_even_a_row,
        without_a_derived_root_nothing_is_written_not_even_a_row
    );

    pg_driver_case!(
        tabular_prepare_pg_the_result_carries_what_the_conversion_reports,
        the_result_carries_what_the_conversion_reports
    );
    pg_driver_case!(
        tabular_prepare_pg_a_second_preparation_is_not_claimed_and_stores_nothing,
        a_second_preparation_is_not_claimed_and_stores_nothing
    );
    pg_driver_case!(
        tabular_prepare_pg_a_file_that_is_not_a_csv_fails_with_a_fixed_text_and_no_cell,
        a_file_that_is_not_a_csv_fails_with_a_fixed_text_and_no_cell
    );
    pg_driver_case!(
        tabular_prepare_pg_a_storage_failure_midway_tracks_every_key_tried_and_deletes_them,
        a_storage_failure_midway_tracks_every_key_tried_and_deletes_them
    );
    pg_driver_case!(
        tabular_prepare_pg_a_manifest_that_could_not_be_stored_never_makes_the_row_ready,
        a_manifest_that_could_not_be_stored_never_makes_the_row_ready
    );
    pg_driver_case!(
        tabular_prepare_pg_a_restart_with_fewer_parts_leaves_stale_keys_the_manifest_never_names,
        a_restart_with_fewer_parts_leaves_stale_keys_the_manifest_never_names
    );
    pg_driver_case!(
        tabular_prepare_pg_the_budget_ends_a_stuck_run_with_the_time_reason_and_removes_its_output,
        the_budget_ends_a_stuck_run_with_the_time_reason_and_removes_its_output
    );
    pg_driver_case!(
        tabular_prepare_pg_a_source_that_never_yields_ends_with_the_time_reason,
        a_source_that_never_yields_ends_with_the_time_reason
    );
    pg_driver_case!(
        tabular_prepare_pg_a_job_whose_row_was_deleted_stops_before_its_next_part_and_leaves_no_row,
        a_job_whose_row_was_deleted_stops_before_its_next_part_and_leaves_no_row
    );
    pg_driver_case!(
        tabular_prepare_pg_a_job_whose_lease_was_taken_stops_and_leaves_the_new_owners_row_and_objects,
        a_job_whose_lease_was_taken_stops_and_leaves_the_new_owners_row_and_objects
    );
    pg_driver_case!(
        tabular_prepare_pg_a_completion_that_finds_the_row_gone_is_cancelled_and_never_ready,
        a_completion_that_finds_the_row_gone_is_cancelled_and_never_ready
    );
    pg_driver_case!(
        tabular_prepare_pg_an_unopenable_source_releases_the_row_without_a_failure,
        an_unopenable_source_releases_the_row_without_a_failure
    );
    pg_driver_case!(
        tabular_prepare_pg_a_source_deleted_during_a_restart_removes_what_was_written_and_releases_the_row,
        a_source_deleted_during_a_restart_removes_what_was_written_and_releases_the_row
    );
    pg_driver_case!(
        tabular_prepare_pg_progress_is_reported_every_interval_to_the_port_and_never_written_to_the_registry,
        progress_is_reported_every_interval_to_the_port_and_never_written_to_the_registry
    );
    pg_driver_case!(
        tabular_prepare_pg_a_finished_preparation_reports_its_final_state_to_the_port,
        a_finished_preparation_reports_its_final_state_to_the_port
    );
    pg_driver_case!(
        tabular_prepare_pg_the_inline_trigger_runs_a_csv_request_through_the_driver,
        the_inline_trigger_runs_a_csv_request_through_the_driver
    );
    pg_driver_case!(
        tabular_prepare_pg_with_the_switch_off_or_another_mime_the_runner_touches_nothing,
        with_the_switch_off_or_another_mime_the_runner_touches_nothing
    );
    pg_driver_case!(
        tabular_prepare_pg_dropping_the_prepare_future_mid_run_leaves_every_stored_object_tracked_in_the_row,
        dropping_the_prepare_future_mid_run_leaves_every_stored_object_tracked_in_the_row
    );
    pg_driver_case!(
        tabular_prepare_pg_a_registry_error_at_completion_leaves_the_stored_objects_tracked_and_not_deleted,
        a_registry_error_at_completion_leaves_the_stored_objects_tracked_and_not_deleted
    );
    pg_driver_case!(
        tabular_prepare_pg_a_registry_error_recording_a_failure_still_deletes_the_objects_that_are_tracked,
        a_registry_error_recording_a_failure_still_deletes_the_objects_that_are_tracked
    );
    pg_driver_case!(
        tabular_prepare_pg_a_part_beyond_the_first_batch_is_tracked_before_its_put_a_batch_ahead,
        a_part_beyond_the_first_batch_is_tracked_before_its_put_a_batch_ahead
    );
    pg_driver_case!(
        tabular_prepare_pg_a_row_that_vanishes_before_the_first_tracking_write_stops_the_job_before_any_object,
        a_row_that_vanishes_before_the_first_tracking_write_stops_the_job_before_any_object
    );
    pg_driver_case!(
        tabular_prepare_pg_a_row_deleted_before_the_budget_ends_the_run_still_gets_its_objects_deleted,
        a_row_deleted_before_the_budget_ends_the_run_still_gets_its_objects_deleted
    );
    pg_driver_case!(
        tabular_prepare_pg_a_row_deleted_before_a_storage_failure_still_gets_its_objects_deleted,
        a_row_deleted_before_a_storage_failure_still_gets_its_objects_deleted
    );
    pg_driver_case!(
        tabular_prepare_pg_a_row_deleted_before_a_source_read_failure_still_gets_its_objects_deleted,
        a_row_deleted_before_a_source_read_failure_still_gets_its_objects_deleted
    );
    pg_driver_case!(
        tabular_prepare_pg_a_lease_taken_before_the_source_turns_out_missing_deletes_nothing_and_releases_nothing,
        a_lease_taken_before_the_source_turns_out_missing_deletes_nothing_and_releases_nothing
    );
    pg_driver_case!(
        tabular_prepare_pg_a_row_taken_before_the_budget_ends_the_run_keeps_the_new_owners_objects,
        a_row_taken_before_the_budget_ends_the_run_keeps_the_new_owners_objects
    );
    pg_driver_case!(
        tabular_prepare_pg_a_manifest_put_that_never_completes_is_ended_by_the_same_budget,
        a_manifest_put_that_never_completes_is_ended_by_the_same_budget
    );
    pg_driver_case!(
        tabular_prepare_pg_a_completion_that_never_returns_is_ended_by_its_own_bound_and_deletes_nothing,
        a_completion_that_never_returns_is_ended_by_its_own_bound_and_deletes_nothing
    );
    pg_driver_case!(
        tabular_prepare_pg_a_delete_that_never_returns_does_not_hold_the_failure_back,
        a_delete_that_never_returns_does_not_hold_the_failure_back
    );
    pg_driver_case!(
        tabular_prepare_pg_the_lease_is_the_budget_plus_the_grace_and_outlasts_the_bounded_job,
        the_lease_is_the_budget_plus_the_grace_and_outlasts_the_bounded_job
    );
    pg_driver_case!(
        tabular_prepare_pg_a_cleanup_of_a_deleted_source_that_does_not_answer_keeps_the_row_and_its_keys,
        a_cleanup_of_a_deleted_source_that_does_not_answer_keeps_the_row_and_its_keys
    );
    pg_case!(
        tabular_prepare_pg_a_deleting_row_is_never_taken_by_an_older_format_claim,
        a_deleting_row_is_never_taken_by_an_older_format_claim
    );
    pg_case!(
        tabular_prepare_pg_an_expired_deleting_lease_can_be_claimed_with_a_fresh_start,
        an_expired_deleting_lease_can_be_claimed_with_a_fresh_start
    );
    pg_case!(
        tabular_prepare_pg_an_older_format_claim_keeps_the_old_manifest_tracked,
        an_older_format_claim_keeps_the_old_manifest_tracked
    );
    pg_case!(
        tabular_prepare_pg_a_ready_row_with_a_missing_manifest_becomes_claimable,
        a_ready_row_with_a_missing_manifest_becomes_claimable
    );
    pg_case!(
        tabular_prepare_pg_mark_manifest_missing_only_touches_ready_rows,
        mark_manifest_missing_only_touches_ready_rows
    );
    pg_case!(
        tabular_prepare_pg_mark_manifest_missing_refuses_a_row_changed_since_the_check,
        mark_manifest_missing_refuses_a_row_changed_since_the_check
    );
    pg_case!(
        tabular_prepare_pg_a_completed_preparation_resets_the_attempts,
        a_completed_preparation_resets_the_attempts
    );
    pg_case!(
        tabular_prepare_pg_a_re_prepared_table_is_not_stale_before_its_first_use,
        a_re_prepared_table_is_not_stale_before_its_first_use
    );
    pg_case!(
        tabular_prepare_pg_delete_if_unchanged_only_deletes_the_row_that_was_observed,
        delete_if_unchanged_only_deletes_the_row_that_was_observed
    );
    pg_case!(
        tabular_prepare_pg_gc_claims_a_row_before_deleting_it,
        gc_claims_a_row_before_deleting_it
    );
    pg_case!(
        tabular_prepare_pg_gc_cannot_claim_a_row_that_changed_since_it_was_read,
        gc_cannot_claim_a_row_that_changed_since_it_was_read
    );
    pg_case!(
        tabular_prepare_pg_gc_cannot_claim_a_row_that_was_used_since_it_was_read,
        gc_cannot_claim_a_row_that_was_used_since_it_was_read
    );
    pg_case!(
        tabular_prepare_pg_an_abandoned_deleting_row_is_found_and_taken_over,
        an_abandoned_deleting_row_is_found_and_taken_over
    );
    pg_case!(
        tabular_prepare_pg_find_stale_selects_old_rows_and_spares_a_live_preparation,
        find_stale_selects_old_rows_and_spares_a_live_preparation
    );
    pg_case!(
        tabular_prepare_pg_find_stale_honours_the_limit,
        find_stale_honours_the_limit
    );
    pg_case!(
        tabular_prepare_pg_list_ready_after_pages_through_ready_rows_only,
        list_ready_after_pages_through_ready_rows_only
    );
    pg_case!(
        tabular_prepare_pg_a_table_in_use_is_not_stale,
        a_table_in_use_is_not_stale
    );
    pg_case!(
        tabular_prepare_pg_find_stale_pages_with_a_keyset_cursor,
        find_stale_pages_with_a_keyset_cursor
    );
    pg_case!(
        tabular_prepare_pg_touch_last_used_marks_use_at_most_once_per_interval,
        touch_last_used_marks_use_at_most_once_per_interval
    );
    pg_case!(
        tabular_prepare_pg_claim_creates_a_running_row_when_there_is_none,
        claim_creates_a_running_row_when_there_is_none
    );
    pg_case!(
        tabular_prepare_pg_claim_is_refused_while_a_lease_is_live,
        claim_is_refused_while_a_lease_is_live
    );
    pg_case!(
        tabular_prepare_pg_claim_takes_over_an_expired_lease,
        claim_takes_over_an_expired_lease
    );
    pg_case!(
        tabular_prepare_pg_a_dead_job_is_retried_only_up_to_the_attempt_cap,
        a_dead_job_is_retried_only_up_to_the_attempt_cap
    );
    pg_case!(
        tabular_prepare_pg_the_lease_boundary_holds_with_sub_second_timestamps,
        the_lease_boundary_holds_with_sub_second_timestamps
    );
    pg_case!(
        tabular_prepare_pg_complete_marks_ready_for_the_lease_owner,
        complete_marks_ready_for_the_lease_owner
    );
    pg_case!(
        tabular_prepare_pg_complete_by_a_non_owner_is_cancelled_and_never_ready,
        complete_by_a_non_owner_is_cancelled_and_never_ready
    );
    pg_case!(
        tabular_prepare_pg_fail_records_the_reason_and_keeps_the_attempt,
        fail_records_the_reason_and_keeps_the_attempt
    );
    pg_case!(
        tabular_prepare_pg_fail_never_writes_for_a_missing_row_or_another_owner,
        fail_never_writes_for_a_missing_row_or_another_owner
    );
    pg_case!(
        tabular_prepare_pg_claim_retries_a_failed_row_until_the_third_attempt,
        claim_retries_a_failed_row_until_the_third_attempt
    );
    pg_case!(
        tabular_prepare_pg_claim_takes_a_row_written_by_an_older_format,
        claim_takes_a_row_written_by_an_older_format
    );
    pg_case!(
        tabular_prepare_pg_a_ready_row_of_the_first_layout_is_claimable_by_the_current_one,
        a_ready_row_of_the_first_layout_is_claimable_by_the_current_one
    );
    pg_case!(
        tabular_prepare_pg_a_ready_row_is_claimable_only_by_a_newer_format,
        a_ready_row_is_claimable_only_by_a_newer_format
    );
    pg_case!(
        tabular_prepare_pg_complete_and_fail_keep_every_blob_ever_recorded,
        complete_and_fail_keep_every_blob_ever_recorded
    );
    pg_case!(
        tabular_prepare_pg_tracked_blobs_are_a_union_written_only_by_the_lease_owner,
        tracked_blobs_are_a_union_written_only_by_the_lease_owner
    );
    pg_case!(
        tabular_prepare_pg_a_non_owner_terminal_write_records_no_blobs,
        a_non_owner_terminal_write_records_no_blobs
    );
    pg_case!(
        tabular_prepare_pg_complete_after_the_row_is_deleted_is_cancelled,
        complete_after_the_row_is_deleted_is_cancelled
    );
    pg_case!(
        tabular_prepare_pg_still_owned_tells_the_owner_from_everyone_else,
        still_owned_tells_the_owner_from_everyone_else
    );
    pg_case!(
        tabular_prepare_pg_a_duplicate_trigger_after_delete_leaves_no_row,
        a_duplicate_trigger_after_delete_leaves_no_row
    );

    #[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
    #[tokio::test]
    async fn tabular_prepare_pg_a_preparation_writes_only_on_claim_and_terminal() {
        a_preparation_writes_only_on_claim_and_terminal(make_registry().await).await;
    }

    #[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
    #[tokio::test]
    async fn tabular_prepare_pg_the_cancellation_check_is_a_read_and_the_lease_is_not_renewed() {
        the_cancellation_check_is_a_read_and_the_lease_is_not_renewed(make_registry().await).await;
    }

    #[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn tabular_prepare_pg_concurrent_claims_have_exactly_one_winner() {
        concurrent_claims_have_exactly_one_winner(Arc::new(make_registry().await)).await;
    }

    #[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
    #[tokio::test]
    async fn tabular_prepare_pg_a_preparation_through_ensure_prepared_writes_only_on_claim_and_terminal(
    ) {
        a_preparation_through_ensure_prepared_writes_only_on_claim_and_terminal(
            make_registry().await,
        )
        .await;
    }
}
