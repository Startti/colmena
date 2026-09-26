//! Where Python code runs. Every caller goes through [`run`]; the executor is
//! chosen once per process from `COLMENA_PYTHON_EXECUTOR`. See
//! docs/developer_guide/53_python_executors.md.

pub mod config;
pub mod frame;
pub mod inprocess;
pub mod protocol;

use crate::dag_engine::domain::python_executor::{
    ExecutorKind, PythonExecutor, PythonRunError, PythonRunRequest, PythonRunResult,
};
use crate::dag_engine::log_policy::T_PYTHON_EXEC;
use config::{ExecutorConfig, ExecutorConfigError, ModesPolicy};
use once_cell::sync::OnceCell;
use std::sync::Arc;
use std::time::{Duration, Instant};

tokio::task_local! {
    static OVERRIDE: Arc<dyn PythonExecutor>;
}

static GLOBAL: OnceCell<Result<Arc<Dispatcher>, ExecutorConfigError>> = OnceCell::new();
static INSTALL_LOGGED: OnceCell<()> = OnceCell::new();

fn global() -> &'static Result<Arc<Dispatcher>, ExecutorConfigError> {
    GLOBAL.get_or_init(|| {
        ExecutorConfig::from_env()
            .and_then(|c| Dispatcher::build(&c))
            .map(Arc::new)
    })
}

/// Build the process executor from the environment (idempotent). Hosts call it
/// at startup so a bad configuration stops the process instead of failing
/// every Python call later. Callers may call this more than once (a host may
/// call it from more than one place, e.g. `EngineConfig::from_env` and the
/// `dag_engine` CLI); the one-time `info!` install event fires only
/// for the first successful call of this function, never on a later one
/// (the executor may already have been built by an earlier [`run`]).
pub fn install_from_env() -> Result<ExecutorKind, ExecutorConfigError> {
    match global() {
        Ok(d) => {
            if INSTALL_LOGGED.set(()).is_ok() {
                tracing::info!(
                    target: T_PYTHON_EXEC,
                    executor = d.isolated.kind().as_str(),
                    modes = d.modes.as_str(),
                    "python executor installed"
                );
            }
            Ok(d.isolated.kind())
        }
        Err(e) => Err(e.clone()),
    }
}

/// Deadline applied to isolated requests that carry none of their own.
pub fn max_timeout() -> Duration {
    match global() {
        Ok(d) => d.max_timeout,
        Err(_) => config::DEFAULT_MAX_TIMEOUT,
    }
}

/// Run Python code with the executor of this task (see [`scope`]) or of the
/// process. A misconfigured process fails every call; it never falls back.
pub async fn run(req: PythonRunRequest) -> Result<PythonRunResult, PythonRunError> {
    if let Ok(exec) = OVERRIDE.try_with(Arc::clone) {
        return exec.run(req).await;
    }
    match global() {
        Ok(d) => d.run(req).await,
        Err(e) => Err(PythonRunError::Internal(format!(
            "PythonExecutorError: the Python executor is misconfigured: {e}"
        ))),
    }
}

/// Test-only override: run `fut` with `exec` as the executor for every
/// [`run`] inside it.
///
/// Skips the dispatcher entirely: no modes policy, no default deadline,
/// no `python run` event — `exec` receives the request exactly as given.
/// This is a `tokio::task_local`, so it does not propagate into a
/// `tokio::spawn`ed task; only the future passed to `scope` (and whatever it
/// calls directly) sees the override.
#[doc(hidden)]
pub async fn scope<F: std::future::Future>(exec: Arc<dyn PythonExecutor>, fut: F) -> F::Output {
    OVERRIDE.scope(exec, fut).await
}

pub(crate) struct Dispatcher {
    modes: ModesPolicy,
    max_timeout: Duration,
    isolated: Arc<dyn PythonExecutor>,
    inprocess: Arc<dyn PythonExecutor>,
}

impl Dispatcher {
    pub(crate) fn new(
        modes: ModesPolicy,
        max_timeout: Duration,
        isolated: Arc<dyn PythonExecutor>,
        inprocess: Arc<dyn PythonExecutor>,
    ) -> Self {
        Self {
            modes,
            max_timeout,
            isolated,
            inprocess,
        }
    }

    pub(crate) fn build(cfg: &ExecutorConfig) -> Result<Self, ExecutorConfigError> {
        let inprocess: Arc<dyn PythonExecutor> = Arc::new(inprocess::InProcessExecutor);
        let isolated: Arc<dyn PythonExecutor> = match cfg.kind {
            ExecutorKind::InProcess => inprocess.clone(),
            other => {
                return Err(ExecutorConfigError(format!(
                    "{}={} is not available in this build",
                    config::ENV_EXECUTOR,
                    other.as_str()
                )))
            }
        };
        Ok(Self::new(cfg.modes, cfg.max_timeout, isolated, inprocess))
    }

    pub(crate) async fn run(
        &self,
        mut req: PythonRunRequest,
    ) -> Result<PythonRunResult, PythonRunError> {
        let isolating = self.isolated.kind() != ExecutorKind::InProcess
            && (req.mode != "none" || self.modes == ModesPolicy::All);
        let exec = if isolating {
            req.timeout.get_or_insert(self.max_timeout);
            &self.isolated
        } else {
            &self.inprocess
        };
        let (mode, code_len, started) = (req.mode.clone(), req.code.len(), Instant::now());
        let out = exec.run(req).await;
        let outcome = match &out {
            Ok(_) => "ok",
            Err(PythonRunError::Python(_)) => "python_error",
            Err(PythonRunError::Timeout) => "timeout",
            Err(PythonRunError::Internal(_)) => "internal",
        };
        // Metadata only — never the code, the inputs, the output or the message.
        tracing::debug!(
            target: T_PYTHON_EXEC,
            executor = exec.kind().as_str(),
            mode = %mode,
            code_len,
            duration_ms = started.elapsed().as_millis() as u64,
            outcome,
            "python run"
        );
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Records what reached it and answers with the request's mode.
    struct Recording {
        kind: ExecutorKind,
        seen: Mutex<Vec<(String, Option<Duration>)>>,
    }

    #[async_trait::async_trait]
    impl PythonExecutor for Recording {
        fn kind(&self) -> ExecutorKind {
            self.kind
        }
        async fn run(&self, req: PythonRunRequest) -> Result<PythonRunResult, PythonRunError> {
            self.seen
                .lock()
                .unwrap()
                .push((req.mode.clone(), req.timeout));
            Ok(PythonRunResult {
                output: Some(serde_json::json!(req.mode)),
                stdout: String::new(),
            })
        }
    }

    fn req(mode: &str, timeout: Option<Duration>) -> PythonRunRequest {
        PythonRunRequest {
            code: "output = 1".into(),
            mode: mode.into(),
            timeout,
            inputs: Default::default(),
        }
    }

    fn rec(kind: ExecutorKind) -> Arc<Recording> {
        Arc::new(Recording {
            kind,
            seen: Mutex::new(vec![]),
        })
    }

    fn pair() -> (Arc<Recording>, Arc<Recording>) {
        (rec(ExecutorKind::Subprocess), rec(ExecutorKind::InProcess))
    }

    #[tokio::test]
    async fn restricted_policy_keeps_none_in_process() {
        let (iso, local) = pair();
        let d = Dispatcher::new(
            ModesPolicy::Restricted,
            Duration::from_secs(99),
            iso.clone(),
            local.clone(),
        );
        d.run(req("none", None)).await.unwrap();
        d.run(req("restricted", Some(Duration::from_secs(3))))
            .await
            .unwrap();
        assert_eq!(
            local.seen.lock().unwrap().as_slice(),
            &[("none".to_string(), None)]
        );
        assert_eq!(
            iso.seen.lock().unwrap().as_slice(),
            &[("restricted".to_string(), Some(Duration::from_secs(3)))]
        );
    }

    #[tokio::test]
    async fn all_policy_isolates_none_and_gives_it_the_default_deadline() {
        let (iso, local) = pair();
        let d = Dispatcher::new(
            ModesPolicy::All,
            Duration::from_secs(99),
            iso.clone(),
            local.clone(),
        );
        d.run(req("none", None)).await.unwrap();
        assert!(local.seen.lock().unwrap().is_empty());
        assert_eq!(
            iso.seen.lock().unwrap().as_slice(),
            &[("none".to_string(), Some(Duration::from_secs(99)))]
        );
    }

    /// Guards `self.isolated.kind() != ExecutorKind::InProcess` (mod.rs run):
    /// when the isolated executor IS the in-process one (no real isolation
    /// configured), the dispatcher must not treat it as isolating — no
    /// default deadline gets forced onto a request that asked for none.
    /// Deleting that guard turns this test red: `req("none", None)` would
    /// then match `ModesPolicy::All` alone, route to `iso` (still an
    /// in-process recorder here) with a forced deadline, and leave `local`
    /// empty with a `Some(_)` timeout recorded instead of `None`.
    #[tokio::test]
    async fn inprocess_isolated_is_not_treated_as_isolating() {
        let iso = rec(ExecutorKind::InProcess);
        let local = rec(ExecutorKind::InProcess);
        let d = Dispatcher::new(
            ModesPolicy::All,
            Duration::from_secs(99),
            iso.clone(),
            local.clone(),
        );
        d.run(req("none", None)).await.unwrap();
        assert!(iso.seen.lock().unwrap().is_empty());
        assert_eq!(
            local.seen.lock().unwrap().as_slice(),
            &[("none".to_string(), None)]
        );
    }

    #[tokio::test]
    async fn scope_overrides_the_global_executor() {
        let (iso, _) = pair();
        let out = scope(iso.clone(), run(req("restricted", None)))
            .await
            .unwrap();
        assert_eq!(out.output, Some(serde_json::json!("restricted")));
        assert_eq!(iso.seen.lock().unwrap().len(), 1);
    }

    #[test]
    fn isolated_kinds_are_refused_until_they_exist() {
        for kind in ["subprocess", "remote"] {
            let cfg = config::ExecutorConfig::from_lookup(|k| {
                (k == config::ENV_EXECUTOR).then(|| kind.to_string())
            })
            .unwrap();
            let err = Dispatcher::build(&cfg).err().expect("must refuse");
            assert!(err.0.contains("not available"), "{err}");
        }
    }

    #[tokio::test]
    async fn inprocess_matches_the_helper() {
        pyo3::Python::initialize();
        let mut inputs = serde_json::Map::new();
        inputs.insert("x".into(), serde_json::json!(20));
        let r = inprocess::InProcessExecutor
            .run(PythonRunRequest {
                code: "print('hi')\noutput = x + 1".into(),
                mode: "restricted".into(),
                timeout: Some(Duration::from_secs(5)),
                inputs,
            })
            .await
            .unwrap();
        assert_eq!(r.output, Some(serde_json::json!(21)));
        assert_eq!(r.stdout, "hi\n");
    }

    #[tokio::test]
    async fn inprocess_keeps_the_helper_error_text() {
        pyo3::Python::initialize();
        let e = inprocess::InProcessExecutor
            .run(PythonRunRequest {
                code: "import os".into(),
                mode: "restricted".into(),
                timeout: Some(Duration::from_secs(5)),
                inputs: Default::default(),
            })
            .await
            .unwrap_err();
        match e {
            PythonRunError::Python(m) => assert!(m.starts_with("SandboxViolation:"), "{m}"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn inprocess_times_out_when_the_deadline_passes() {
        pyo3::Python::initialize();
        let e = inprocess::InProcessExecutor
            .run(PythonRunRequest {
                code: "import time\ntime.sleep(2)".into(),
                mode: "none".into(),
                timeout: Some(Duration::from_millis(100)),
                inputs: Default::default(),
            })
            .await
            .unwrap_err();
        assert!(matches!(e, PythonRunError::Timeout), "{e:?}");
    }
}
