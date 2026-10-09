//! Ports of the preparation lifecycle, with the defaults used locally and in
//! tests. The host (ADP) replaces them: its trigger queues the item and runs
//! the preparation job, its progress adapter keeps the status in Redis.
//! Colmena itself has no Redis dependency.

use crate::dag_engine::engine::parse_bool_str;
use async_trait::async_trait;
use std::sync::Arc;
use thiserror::Error;

/// What the host needs to prepare one source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrepareRequest {
    pub source_key: String,
    pub mime_type: String,
    pub filename: String,
    pub size_bytes: u64,
}

#[derive(Debug, Error)]
pub enum PrepareTriggerError {
    #[error("could not request the preparation: {0}")]
    Unavailable(String),
}

/// Asks the host to start preparing a source. Best-effort and idempotent:
/// the preparation itself claims the registry row, so a duplicate request
/// results in one preparation.
#[async_trait]
pub trait PrepareTrigger: Send + Sync {
    async fn request(&self, req: PrepareRequest) -> Result<(), PrepareTriggerError>;

    /// Whether a request can actually lead to a preparation. `false` only for
    /// the placeholder default that has no converter behind it.
    fn is_wired(&self) -> bool {
        true
    }
}

/// The in-process converter [`InlineTrigger`] hands requests to. The real
/// one arrives with the conversion slice; until then the default only logs.
#[async_trait]
pub trait PrepareRunner: Send + Sync {
    async fn run(&self, req: PrepareRequest);
}

struct NotWiredRunner;

#[async_trait]
impl PrepareRunner for NotWiredRunner {
    async fn run(&self, req: PrepareRequest) {
        tracing::warn!(
            target: "colmena::tabular_prepare",
            source_key = %req.source_key,
            "no tabular converter is wired; the preparation request was dropped"
        );
    }
}

/// Default trigger for local runs and tests: runs the request in this
/// process, detached from the caller.
pub struct InlineTrigger {
    runner: Arc<dyn PrepareRunner>,
    wired: bool,
}

impl InlineTrigger {
    pub fn new(runner: Arc<dyn PrepareRunner>) -> Self {
        Self {
            runner,
            wired: true,
        }
    }
}

impl Default for InlineTrigger {
    fn default() -> Self {
        Self {
            runner: Arc::new(NotWiredRunner),
            wired: false,
        }
    }
}

#[async_trait]
impl PrepareTrigger for InlineTrigger {
    async fn request(&self, req: PrepareRequest) -> Result<(), PrepareTriggerError> {
        let runner = self.runner.clone();
        tokio::spawn(async move { runner.run(req).await });
        Ok(())
    }

    fn is_wired(&self) -> bool {
        self.wired
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressState {
    Queued,
    Running,
    Ready,
    Failed,
    Cancelled,
}

/// Progress of one preparation. It lives outside the registry: the registry
/// is written only on state change, never for progress.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrepareProgressInfo {
    pub state: ProgressState,
    pub done: u64,
    pub total: Option<u64>,
}

#[async_trait]
pub trait PrepareProgress: Send + Sync {
    async fn report(&self, source_key: &str, info: PrepareProgressInfo);
    async fn read(&self, source_key: &str) -> Option<PrepareProgressInfo>;
}

/// Default progress: reports go nowhere and nothing can be read back.
pub struct NoopProgress;

#[async_trait]
impl PrepareProgress for NoopProgress {
    async fn report(&self, _source_key: &str, _info: PrepareProgressInfo) {}

    async fn read(&self, _source_key: &str) -> Option<PrepareProgressInfo> {
        None
    }
}

/// Engine-level configuration of the feature, held in `EngineConfig`.
#[derive(Clone)]
pub struct PrepareConfig {
    /// `COLMENA_LARGE_TABULAR=on`, read once. Off by default: nothing in this
    /// module runs and behaviour is unchanged.
    pub large_tabular: bool,
    pub trigger: Arc<dyn PrepareTrigger>,
    pub progress: Arc<dyn PrepareProgress>,
    /// The preparation registry the host keeps, which the large path reads to
    /// vouch for a prepared copy before it runs code over it. Without one the
    /// engine can register a large file but cannot analyse it.
    pub registry: Option<Arc<dyn crate::tabular_prepare::registry::PreparationRegistry>>,
}

impl PrepareConfig {
    /// Build the config from the raw value of `COLMENA_LARGE_TABULAR`.
    pub fn from_switch(raw: Option<&str>) -> Self {
        Self {
            large_tabular: raw.and_then(parse_bool_str).unwrap_or(false),
            ..Self::default()
        }
    }

    /// Refuse a configuration that can only wait forever: the switch on with
    /// a trigger that has no converter behind it. Checked when the engine
    /// starts, so a missing runner is a start-up error, not a silent
    /// `StillPreparing` on every call.
    pub fn validate(&self) -> Result<(), String> {
        if self.large_tabular && !self.trigger.is_wired() {
            return Err(
                "COLMENA_LARGE_TABULAR is on but no preparation trigger or runner is \
                 wired; set EngineConfig.prepare.trigger before starting the engine"
                    .to_string(),
            );
        }
        Ok(())
    }

    /// Read `COLMENA_LARGE_TABULAR` from the environment.
    pub fn from_env() -> Self {
        Self::from_switch(std::env::var("COLMENA_LARGE_TABULAR").ok().as_deref())
    }
}

impl Default for PrepareConfig {
    fn default() -> Self {
        Self {
            large_tabular: false,
            trigger: Arc::new(InlineTrigger::default()),
            progress: Arc::new(NoopProgress),
            registry: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tokio::sync::oneshot;

    fn request(key: &str) -> PrepareRequest {
        PrepareRequest {
            source_key: key.to_string(),
            mime_type: "text/csv".to_string(),
            filename: "sales.csv".to_string(),
            size_bytes: 60_000_000,
        }
    }

    #[test]
    fn tabular_prepare_config_default_is_off() {
        assert!(!PrepareConfig::default().large_tabular);
    }

    #[test]
    fn tabular_prepare_switch_accepts_on_and_the_usual_truthy_words() {
        for raw in ["on", "ON", " on ", "true", "1", "yes"] {
            assert!(
                PrepareConfig::from_switch(Some(raw)).large_tabular,
                "{raw:?} should enable"
            );
        }
    }

    #[test]
    fn tabular_prepare_switch_is_off_when_unset_off_or_unrecognised() {
        for raw in [
            None,
            Some("off"),
            Some("0"),
            Some("false"),
            Some(""),
            Some("maybe"),
        ] {
            assert!(
                !PrepareConfig::from_switch(raw).large_tabular,
                "{raw:?} should not enable"
            );
        }
    }

    #[tokio::test]
    async fn tabular_prepare_noop_progress_remembers_nothing() {
        let progress = NoopProgress;
        progress
            .report(
                "k",
                PrepareProgressInfo {
                    state: ProgressState::Running,
                    done: 5,
                    total: Some(10),
                },
            )
            .await;
        assert_eq!(progress.read("k").await, None);
    }

    struct RecordingRunner {
        seen: Mutex<Vec<PrepareRequest>>,
        done: Mutex<Option<oneshot::Sender<()>>>,
    }

    #[async_trait::async_trait]
    impl PrepareRunner for RecordingRunner {
        async fn run(&self, req: PrepareRequest) {
            self.seen.lock().unwrap().push(req);
            if let Some(tx) = self.done.lock().unwrap().take() {
                let _ = tx.send(());
            }
        }
    }

    #[tokio::test]
    async fn tabular_prepare_inline_trigger_runs_the_request_once_in_this_process() {
        let (tx, rx) = oneshot::channel();
        let runner = Arc::new(RecordingRunner {
            seen: Mutex::new(Vec::new()),
            done: Mutex::new(Some(tx)),
        });
        let trigger = InlineTrigger::new(runner.clone());
        trigger.request(request("a.csv")).await.unwrap();
        rx.await.unwrap();
        let seen = runner.seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].source_key, "a.csv");
        assert_eq!(seen[0].size_bytes, 60_000_000);
    }

    #[tokio::test]
    async fn tabular_prepare_default_inline_trigger_accepts_a_request_without_a_converter() {
        // No converter exists yet: the default runner only logs, so requesting
        // is harmless and never an error.
        let cfg = PrepareConfig::default();
        assert!(cfg.trigger.request(request("b.csv")).await.is_ok());
    }

    #[test]
    fn tabular_prepare_validate_refuses_the_switch_on_without_a_wired_runner() {
        let on = PrepareConfig::from_switch(Some("on"));
        let err = on.validate().unwrap_err();
        assert!(err.contains("COLMENA_LARGE_TABULAR"), "{err}");
        // Switch off needs nothing wired; a wired trigger satisfies the switch.
        assert!(PrepareConfig::default().validate().is_ok());
        let wired = PrepareConfig {
            large_tabular: true,
            trigger: Arc::new(InlineTrigger::new(Arc::new(RecordingRunner {
                seen: Mutex::new(Vec::new()),
                done: Mutex::new(None),
            }))),
            ..PrepareConfig::default()
        };
        assert!(wired.validate().is_ok());
    }
}
