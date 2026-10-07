//! The seam between what a call over prepared tables needs and how an executor
//! takes it. `MountedExecutor::run_with_mounts` is the whole contract: given the
//! code and the verified tables, the executor makes the tables readable at
//! `/data` for the call, runs it, and answers with the result or a refusal.
//!
//! Where the tables go is the executor's business and stays behind this trait:
//! the subprocess executor copies them into the call's data directory on its own
//! host (`local`); a remote executor would carry them to its host on one
//! streamed request (the `/v2/run` protocol of the design, NOT built: see
//! `remote_seam`). Nothing above this trait knows which.

use super::refusal::RunRefusal;
use super::stage::{StageLimits, Staged};
use super::verify::PreparedTables;
use crate::dag_engine::domain::python_executor::{
    PythonRunError, PythonRunRequest, PythonRunResult,
};
use crate::storage::domain::OutputStorageRepository;
use async_trait::async_trait;

/// Size of the call's output volume, in MiB. An estimate (`OUT_MAX` waits for
/// the instance measurement, spike item 5): the executor's own bound is 1,024.
pub const OUT_MIB: u64 = 256;

/// What a call over prepared tables carries besides its code.
pub struct MountedCall<'a> {
    pub storage: &'a dyn OutputStorageRepository,
    pub plan: &'a PreparedTables,
    /// Table indexes (as in the manifest) to make readable.
    pub tables: &'a [usize],
    pub limits: StageLimits,
    /// Size of the output volume, in MiB.
    pub out_mb: u64,
}

/// A call that ran.
#[derive(Debug, PartialEq)]
pub struct MountedResult {
    pub result: PythonRunResult,
    pub staged: Staged,
}

/// Why a call did not produce a result.
#[derive(Debug, PartialEq)]
pub enum MountedError {
    /// Refused before any code ran: a typed refusal for the model.
    Refused(RunRefusal),
    /// The code ran (or could not be run) and the executor reports it as it
    /// does for any call: Python error text, timeout, or an executor failure.
    Run(PythonRunError),
}

#[async_trait]
pub trait MountedExecutor: Send + Sync {
    async fn run_with_mounts(
        &self,
        req: PythonRunRequest,
        call: MountedCall<'_>,
    ) -> Result<MountedResult, MountedError>;
}
