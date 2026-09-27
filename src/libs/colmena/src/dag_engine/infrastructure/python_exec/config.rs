//! Process-wide configuration of the Python executor, read from environment
//! variables once. An invalid value is an error, never a silent default.

use crate::dag_engine::domain::python_executor::ExecutorKind;
use std::path::PathBuf;
use std::time::Duration;

pub const ENV_EXECUTOR: &str = "COLMENA_PYTHON_EXECUTOR";
pub const ENV_MODES: &str = "COLMENA_PYTHON_EXECUTOR_MODES";
pub const ENV_MAX_TIMEOUT: &str = "COLMENA_PYTHON_EXECUTOR_MAX_TIMEOUT_SECS";
pub const DEFAULT_MAX_TIMEOUT: Duration = Duration::from_secs(3600);
pub const ENV_BIN: &str = "COLMENA_PYTHON_EXECUTOR_BIN";
pub const ENV_SLOTS: &str = "COLMENA_PYTHON_EXECUTOR_SLOTS";
pub const ENV_MEMORY_MB: &str = "COLMENA_PYTHON_EXECUTOR_MEMORY_MB";
pub const ENV_HIDE_PATHS: &str = "COLMENA_PYTHON_EXECUTOR_HIDE_PATHS";
pub const ENV_MAX_REQUEST_MB: &str = "COLMENA_PYTHON_EXECUTOR_MAX_REQUEST_MB";
pub const ENV_MAX_RESPONSE_MB: &str = "COLMENA_PYTHON_EXECUTOR_MAX_RESPONSE_MB";
const MIB: usize = 1024 * 1024;

/// Which sandbox modes go to an isolated executor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModesPolicy {
    /// Only `restricted`; `none` keeps running in this process.
    Restricted,
    /// Every mode, `none` included.
    All,
}

impl ModesPolicy {
    /// The env value that produces this variant (`COLMENA_PYTHON_EXECUTOR_MODES`).
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            ModesPolicy::Restricted => "restricted",
            ModesPolicy::All => "all",
        }
    }
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
    pub subprocess: SubprocessConfig,
}

/// Settings of the subprocess executor (Linux).
#[derive(Debug, Clone, PartialEq)]
pub struct SubprocessConfig {
    pub bin: PathBuf,
    pub slots: usize,
    /// Memory each child may use on top of the warm template it forks from.
    pub memory_mb: u64,
    pub uid_base: u32,
    pub tmp_mb: u64,
    pub hide_paths: Vec<PathBuf>,
    pub max_request_bytes: usize,
    pub max_response_bytes: usize,
}

pub(crate) fn invalid(var: &str, value: &str, expected: &str) -> ExecutorConfigError {
    ExecutorConfigError(format!("{var}={value:?} is invalid; expected {expected}"))
}

fn parse_in<T: std::str::FromStr + PartialOrd>(
    get: &impl Fn(&str) -> Option<String>,
    var: &str,
    default: T,
    min: T,
    max: T,
) -> Result<T, ExecutorConfigError> {
    match get(var)
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
    {
        None => Ok(default),
        Some(v) => match v.parse::<T>() {
            Ok(n) if n >= min && n <= max => Ok(n),
            _ => Err(invalid(var, &v, "a number in the supported range")),
        },
    }
}

impl SubprocessConfig {
    pub fn from_lookup(get: &impl Fn(&str) -> Option<String>) -> Result<Self, ExecutorConfigError> {
        let bin = match get(ENV_BIN)
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
        {
            Some(p) => PathBuf::from(p),
            None => std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(|d| d.join("python_executor")))
                .unwrap_or_else(|| PathBuf::from("python_executor")),
        };
        let cores = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(2);
        Ok(Self {
            bin,
            slots: parse_in(get, ENV_SLOTS, cores.min(8), 1, 64)?,
            memory_mb: parse_in(get, ENV_MEMORY_MB, 2048, 256, 65536)?,
            uid_base: 20000,
            tmp_mb: 64,
            hide_paths: get(ENV_HIDE_PATHS)
                .map(|v| {
                    v.split(':')
                        .map(str::trim)
                        .filter(|p| !p.is_empty())
                        .map(PathBuf::from)
                        .collect()
                })
                .unwrap_or_default(),
            max_request_bytes: parse_in(get, ENV_MAX_REQUEST_MB, 256usize, 1, 4095)? * MIB,
            max_response_bytes: parse_in(get, ENV_MAX_RESPONSE_MB, 256usize, 1, 4095)? * MIB,
        })
    }
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
            subprocess: SubprocessConfig::from_lookup(&get)?,
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
    fn subprocess_defaults_and_overrides() {
        let c = cfg(&[]).unwrap().subprocess;
        assert_eq!(c.memory_mb, 2048);
        assert_eq!(c.max_request_bytes, 256 * 1024 * 1024);
        assert!(c.bin.ends_with("python_executor"));
        let c = cfg(&[
            ("COLMENA_PYTHON_EXECUTOR_SLOTS", "3"),
            ("COLMENA_PYTHON_EXECUTOR_MEMORY_MB", "1024"),
            ("COLMENA_PYTHON_EXECUTOR_HIDE_PATHS", "/data:/etc/extra"),
        ])
        .unwrap()
        .subprocess;
        assert_eq!((c.slots, c.memory_mb), (3, 1024));
        assert_eq!(
            c.hide_paths,
            vec![PathBuf::from("/data"), PathBuf::from("/etc/extra")]
        );
        assert!(cfg(&[("COLMENA_PYTHON_EXECUTOR_SLOTS", "0")]).is_err());
        assert!(cfg(&[("COLMENA_PYTHON_EXECUTOR_MEMORY_MB", "100")]).is_err());
    }

    #[test]
    fn hidden_paths_are_trimmed() {
        let c = cfg(&[(ENV_HIDE_PATHS, " /data : /x ")]).unwrap().subprocess;
        assert_eq!(
            c.hide_paths,
            vec![PathBuf::from("/data"), PathBuf::from("/x")]
        );
    }

    #[test]
    fn modes_names_are_the_env_values() {
        assert_eq!(ModesPolicy::Restricted.as_str(), "restricted");
        assert_eq!(ModesPolicy::All.as_str(), "all");
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
