//! Process-wide configuration of the Python executor, read from environment
//! variables once. An invalid value is an error, never a silent default.

use crate::dag_engine::domain::python_executor::ExecutorKind;
use std::time::Duration;

pub const ENV_EXECUTOR: &str = "COLMENA_PYTHON_EXECUTOR";
pub const ENV_MODES: &str = "COLMENA_PYTHON_EXECUTOR_MODES";
pub const ENV_MAX_TIMEOUT: &str = "COLMENA_PYTHON_EXECUTOR_MAX_TIMEOUT_SECS";
pub const DEFAULT_MAX_TIMEOUT: Duration = Duration::from_secs(3600);

/// Which sandbox modes go to an isolated executor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModesPolicy {
    /// Only `restricted`; `none` keeps running in this process.
    Restricted,
    /// Every mode, `none` included.
    All,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ExecutorConfigError(pub String);

#[derive(Debug, Clone, PartialEq)]
pub struct ExecutorConfig {
    pub kind: ExecutorKind,
    pub modes: ModesPolicy,
    /// Deadline for requests that carry none of their own.
    pub max_timeout: Duration,
}

pub(crate) fn invalid(var: &str, value: &str, expected: &str) -> ExecutorConfigError {
    ExecutorConfigError(format!("{var}={value:?} is invalid; expected {expected}"))
}

impl ExecutorConfig {
    pub fn from_env() -> Result<Self, ExecutorConfigError> {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    /// Pure core of [`Self::from_env`], testable without touching the process
    /// environment.
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self, ExecutorConfigError> {
        let val = |k: &str| {
            get(k)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        let kind = match val(ENV_EXECUTOR).as_deref() {
            None | Some("inprocess") => ExecutorKind::InProcess,
            Some("subprocess") => ExecutorKind::Subprocess,
            Some("remote") => ExecutorKind::Remote,
            Some(v) => return Err(invalid(ENV_EXECUTOR, v, "inprocess, subprocess or remote")),
        };
        let modes = match val(ENV_MODES).as_deref() {
            None | Some("restricted") => ModesPolicy::Restricted,
            Some("all") => ModesPolicy::All,
            Some(v) => return Err(invalid(ENV_MODES, v, "restricted or all")),
        };
        let max_timeout = match val(ENV_MAX_TIMEOUT) {
            None => DEFAULT_MAX_TIMEOUT,
            Some(v) => match v.parse::<u64>() {
                Ok(n) if n > 0 => Duration::from_secs(n),
                _ => return Err(invalid(ENV_MAX_TIMEOUT, &v, "a positive number of seconds")),
            },
        };
        Ok(Self {
            kind,
            modes,
            max_timeout,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn cfg(pairs: &[(&str, &str)]) -> Result<ExecutorConfig, ExecutorConfigError> {
        let m: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        ExecutorConfig::from_lookup(|k| m.get(k).cloned())
    }

    #[test]
    fn unset_means_inprocess_restricted_one_hour() {
        let c = cfg(&[]).unwrap();
        assert_eq!(c.kind, ExecutorKind::InProcess);
        assert_eq!(c.modes, ModesPolicy::Restricted);
        assert_eq!(c.max_timeout, DEFAULT_MAX_TIMEOUT);
    }

    #[test]
    fn blank_is_the_same_as_unset() {
        assert_eq!(
            cfg(&[(ENV_EXECUTOR, "  ")]).unwrap().kind,
            ExecutorKind::InProcess
        );
    }

    #[test]
    fn known_values_parse() {
        assert_eq!(
            cfg(&[(ENV_EXECUTOR, "subprocess")]).unwrap().kind,
            ExecutorKind::Subprocess
        );
        assert_eq!(
            cfg(&[(ENV_EXECUTOR, "remote")]).unwrap().kind,
            ExecutorKind::Remote
        );
        assert_eq!(cfg(&[(ENV_MODES, "all")]).unwrap().modes, ModesPolicy::All);
        assert_eq!(
            cfg(&[(ENV_MAX_TIMEOUT, "120")]).unwrap().max_timeout,
            Duration::from_secs(120)
        );
    }

    #[test]
    fn a_typo_is_an_error_not_a_fallback() {
        let e = cfg(&[(ENV_EXECUTOR, "remtoe")]).unwrap_err();
        assert!(e.0.contains("COLMENA_PYTHON_EXECUTOR"), "{e}");
        assert!(cfg(&[(ENV_MODES, "everything")]).is_err());
        assert!(cfg(&[(ENV_MAX_TIMEOUT, "0")]).is_err());
        assert!(cfg(&[(ENV_MAX_TIMEOUT, "abc")]).is_err());
    }
}
