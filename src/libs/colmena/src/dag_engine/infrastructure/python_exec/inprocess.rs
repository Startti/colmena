//! Runs Python inside this process, exactly as before the executor port
//! existed: a blocking task plus a deadline that stops waiting (a Python loop
//! holding the GIL keeps its thread busy after the deadline).

use crate::dag_engine::domain::python_executor::{
    ExecutorKind, PythonExecutor, PythonRunError, PythonRunRequest, PythonRunResult,
};
use crate::dag_engine::infrastructure::nodes::python_node::execute_sandboxed_helper;
use async_trait::async_trait;

pub struct InProcessExecutor;

#[async_trait]
impl PythonExecutor for InProcessExecutor {
    fn kind(&self) -> ExecutorKind {
        ExecutorKind::InProcess
    }

    async fn run(&self, req: PythonRunRequest) -> Result<PythonRunResult, PythonRunError> {
        let PythonRunRequest {
            code,
            mode,
            timeout,
            inputs,
        } = req;
        let secs = timeout.map(|t| t.as_secs()).unwrap_or(0);
        let task = tokio::task::spawn_blocking(move || {
            execute_sandboxed_helper(&code, &mode, secs, &inputs)
        });
        let joined = match timeout {
            Some(t) => tokio::time::timeout(t, task)
                .await
                .map_err(|_| PythonRunError::Timeout)?,
            None => task.await,
        };
        match joined {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(message)) => Err(PythonRunError::Python(message)),
            Err(join) => Err(PythonRunError::Internal(format!(
                "internal join error: {join}"
            ))),
        }
    }
}
