//! Mounted runs on the remote executor: NOT built.
//!
//! The design carries the tables to a remote executor on one streamed HTTP/2
//! request (`POST /v2/run`, multipart, the code and the parts in one body, the
//! executor staging them on its own host). Whether a Cloud Run service accepts
//! a 60 MiB to 1 GiB request body that way is spike item 3, which needs the
//! deployed executor and a granted identity and could not be measured here. So
//! this implementation refuses, with a typed reason, instead of guessing at a
//! wire: a remote executor never silently runs a large file some other way.
//! The transport belongs behind [`MountedExecutor`]; when item 3 passes, it is
//! implemented here and nothing above the trait changes.

use super::mounted::{MountedCall, MountedError, MountedExecutor, MountedResult};
use super::refusal::{RunRefusal, Unavailable};
use crate::dag_engine::domain::python_executor::PythonRunRequest;
use crate::dag_engine::infrastructure::python_exec::remote::RemoteExecutor;
use async_trait::async_trait;

#[async_trait]
impl MountedExecutor for RemoteExecutor {
    async fn run_with_mounts(
        &self,
        _req: PythonRunRequest,
        _call: MountedCall<'_>,
    ) -> Result<MountedResult, MountedError> {
        Err(MountedError::Refused(RunRefusal::Unavailable(
            Unavailable::Unsupported,
        )))
    }
}
