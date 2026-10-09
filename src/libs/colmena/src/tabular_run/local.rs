//! Mounted runs on the subprocess executor (Linux). The call's tables are
//! copied into its own `data` directory and bound read-only at `/data` by the
//! jail; see docs/developer_guide/53_python_executors.md.
//!
//! Staging goes through [`SubprocessExecutor::stage_call`], the budgeted path:
//! the unbudgeted `StagedCall::create` and `run_staged` primitives are never
//! used to take a call's volume.

use super::collect::{collect_out, CollectLimits};
use super::mounted::{MountedCall, MountedError, MountedExecutor, MountedResult};
use super::refusal::{Budget, RunRefusal, Unavailable};
use super::stage::stage_tables;
use super::volume::Volume;
use crate::dag_engine::domain::python_executor::PythonRunRequest;
use crate::dag_engine::infrastructure::python_exec::staging::StageError;
use crate::dag_engine::infrastructure::python_exec::subprocess::SubprocessExecutor;
use crate::dag_engine::log_policy::T_PYTHON_EXEC;
use async_trait::async_trait;

/// What the model is told when the executor could not make the call's volume.
/// The executor's own text is logged and never shown.
pub fn refuse_stage(e: &StageError) -> RunRefusal {
    match e {
        StageError::OverBudget { .. } => RunRefusal::OverBudget(Budget::Volumes),
        StageError::NoStagingRoot => RunRefusal::Unavailable(Unavailable::NoStagingRoot),
        // The executor started without run mounts (the root's lock, its capabilities):
        // it says so instead of staging calls it cannot serve.
        StageError::MountsDisabled(_) => RunRefusal::Unavailable(Unavailable::MountsDisabled),
        // An output size the executor itself refuses can never work: setup, not a wait.
        StageError::InvalidSize(_) => RunRefusal::Unavailable(Unavailable::Misconfigured),
        StageError::Io(_) => RunRefusal::Unavailable(Unavailable::Executor),
    }
}

#[async_trait]
impl MountedExecutor for SubprocessExecutor {
    async fn run_with_mounts(
        &self,
        req: PythonRunRequest,
        call: MountedCall<'_>,
    ) -> Result<MountedResult, MountedError> {
        let staged_call = Volume::new(self.stage_call(call.out_mb).map_err(|e| {
            tracing::warn!(target: T_PYTHON_EXEC, error = %e, "could not stage a call with mounts");
            MountedError::Refused(refuse_stage(&e))
        })?);
        let outcome = async {
            let staged = stage_tables(
                call.storage,
                call.plan,
                call.tables,
                &staged_call.get().data_dir(),
                call.limits,
            )
            .await
            .map_err(MountedError::Refused)?;
            let result = self
                .run_staged(req, staged_call.get().mounts())
                .await
                .map_err(MountedError::Run)?;
            // The child is dead (SIGKILL to its uid before `run_staged` returns)
            // and the volume is still mounted: read what it wrote, nothing else.
            let mut done = MountedResult {
                result,
                staged,
                emitted: vec![],
                rejected: vec![],
                too_many_entries: false,
            };
            if let Some(sink) = call.sink {
                let found = collect_out(&staged_call.get().out_dir(), CollectLimits::default())
                    .map_err(|_| {
                        MountedError::Refused(RunRefusal::Unavailable(Unavailable::Executor))
                    })?;
                done.rejected = found.rejected;
                done.too_many_entries = found.too_many_entries;
                for file in found.files {
                    let name = file.name.clone();
                    sink.accept(file).await.map_err(MountedError::Refused)?;
                    done.emitted.push(name);
                }
            }
            Ok(done)
        }
        .await;
        // The volume is unmounted and the directories removed off the async
        // worker, and before this returns, so the budget share is given back.
        // Back on the blocking pool and before this returns; a path that never
        // gets here (an error, a panic, the future dropped) goes through `Drop`,
        // which hands it to the same pool.
        staged_call.release().await;
        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    #[test]
    fn a_stage_failure_is_refused_with_fixed_text_never_the_executors() {
        let over = StageError::OverBudget {
            volumes: 2,
            mib: 2048,
            max_volumes: 2,
            max_mib: 2048,
        };
        assert_eq!(refuse_stage(&over), RunRefusal::OverBudget(Budget::Volumes));
        assert_eq!(
            refuse_stage(&StageError::NoStagingRoot),
            RunRefusal::Unavailable(Unavailable::NoStagingRoot)
        );
        let io = StageError::Io(io::Error::other("/var/lib/secret/path"));
        let refusal = refuse_stage(&io);
        assert_eq!(refusal, RunRefusal::Unavailable(Unavailable::Executor));
        assert!(!refusal.message().contains("secret"));
        assert_eq!(
            refuse_stage(&StageError::InvalidSize(0)),
            RunRefusal::Unavailable(Unavailable::Misconfigured)
        );
        assert!(!refuse_stage(&StageError::InvalidSize(0)).retryable());
        assert!(!refuse_stage(&StageError::NoStagingRoot).retryable());
        assert!(refuse_stage(&over).retryable());
    }
}
