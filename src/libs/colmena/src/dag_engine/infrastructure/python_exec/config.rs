//! Process-wide configuration of the Python executor, read from environment
//! variables once. An invalid value is an error, never a silent default.

use crate::dag_engine::domain::python_executor::ExecutorKind;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

pub const ENV_EXECUTOR: &str = "COLMENA_PYTHON_EXECUTOR";
pub const ENV_MODES: &str = "COLMENA_PYTHON_EXECUTOR_MODES";
pub const ENV_MAX_TIMEOUT: &str = "COLMENA_PYTHON_EXECUTOR_MAX_TIMEOUT_SECS";
pub const DEFAULT_MAX_TIMEOUT: Duration = Duration::from_secs(3600);
pub const ENV_BIN: &str = "COLMENA_PYTHON_EXECUTOR_BIN";
pub const ENV_SLOTS: &str = "COLMENA_PYTHON_EXECUTOR_SLOTS";
pub const ENV_MEMORY_MB: &str = "COLMENA_PYTHON_EXECUTOR_MEMORY_MB";
pub const ENV_HIDE_PATHS: &str = "COLMENA_PYTHON_EXECUTOR_HIDE_PATHS";
/// Dark switch of the large tabular feature; read here only to gate staging.
pub const ENV_LARGE_TABULAR: &str = "COLMENA_LARGE_TABULAR";
pub const ENV_STAGING_DIR: &str = "COLMENA_PYTHON_EXECUTOR_STAGING_DIR";
pub const ENV_MAX_REQUEST_MB: &str = "COLMENA_PYTHON_EXECUTOR_MAX_REQUEST_MB";
pub const ENV_MAX_RESPONSE_MB: &str = "COLMENA_PYTHON_EXECUTOR_MAX_RESPONSE_MB";
pub const ENV_REFUSE_OUTPUT: &str = "COLMENA_PYTHON_EXECUTOR_REFUSE_OUTPUT";
pub const ENV_URL: &str = "COLMENA_PYTHON_EXECUTOR_URL";
pub const ENV_AUTH: &str = "COLMENA_PYTHON_EXECUTOR_AUTH";
pub const ENV_TOKEN_FILE: &str = "COLMENA_PYTHON_EXECUTOR_TOKEN_FILE";
pub const ENV_AUDIENCE: &str = "COLMENA_PYTHON_EXECUTOR_AUDIENCE";
pub const ENV_MAX_WIRE_MB: &str = "COLMENA_PYTHON_EXECUTOR_MAX_WIRE_MB";
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
    /// Set when `COLMENA_PYTHON_EXECUTOR_URL` is.
    pub remote: Option<RemoteConfig>,
}

/// How the remote executor authenticates each call.
#[derive(Debug, Clone, PartialEq)]
pub enum RemoteAuthConfig {
    None,
    /// `Authorization: Bearer` with the file's content, trimmed.
    BearerFile(PathBuf),
    /// An identity token for `audience` from the GCP metadata server.
    GcpIdToken {
        audience: String,
    },
}

/// Settings of the remote executor, a client of `python_executor serve`.
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteConfig {
    pub url: reqwest::Url,
    pub auth: RemoteAuthConfig,
    pub max_request_bytes: usize,
    pub max_response_bytes: usize,
    /// Cap on the compressed body; `None` is no cap. Set it when the
    /// transport in front of the service limits request size (HTTP/1 fronts
    /// commonly cap at 32 MiB), so an oversized call fails with a clear
    /// message.
    pub max_wire_bytes: Option<usize>,
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
    /// Where calls that carry prepared data are staged: `Some` only with
    /// `COLMENA_LARGE_TABULAR` on and `COLMENA_PYTHON_EXECUTOR_STAGING_DIR` set.
    pub staging_root: Option<PathBuf>,
    pub max_request_bytes: usize,
    pub max_response_bytes: usize,
    /// Literals a result may not contain, e.g. the prefix of a credential
    /// type: a result with any of them does not leave the executor. A second
    /// layer: matched byte for byte against the encoded result, so an
    /// encoded or transformed value does not match; the in-process executor
    /// does not apply it.
    pub refuse_output: Vec<String>,
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

/// Whether any component of `p` is `..`. A `..` component would let a
/// configured path walk back out of what it looks like it names lexically
/// (`/home/../etc/app.key`), which the self-test's nested-path exclusion
/// reasons about by prefix, not by resolving the path.
fn has_parent_dir_component(p: &Path) -> bool {
    p.components().any(|c| c == Component::ParentDir)
}

/// Absolute paths separated by `:`, none with a `..` component. A relative
/// one would be resolved against whatever directory the executor started in,
/// not the path meant.
fn hide_paths(get: &impl Fn(&str) -> Option<String>) -> Result<Vec<PathBuf>, ExecutorConfigError> {
    let Some(v) = get(ENV_HIDE_PATHS) else {
        return Ok(Vec::new());
    };
    let paths: Vec<PathBuf> = v
        .split(':')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .collect();
    if paths
        .iter()
        .all(|p| p.is_absolute() && !has_parent_dir_component(p))
    {
        return Ok(paths);
    }
    Err(invalid(
        ENV_HIDE_PATHS,
        &v,
        "absolute paths separated by ':', none with a '..' component",
    ))
}

/// The staging root, canonical, only while `COLMENA_LARGE_TABULAR` is on: with
/// the switch off the directory is not even looked at, so today's executor
/// starts exactly as before. The directory must exist, hold no `..` and be a
/// real directory; it is resolved once here and walked without following links
/// by the jail on every call.
fn staging_root(
    get: &impl Fn(&str) -> Option<String>,
) -> Result<Option<PathBuf>, ExecutorConfigError> {
    let on = get(ENV_LARGE_TABULAR).and_then(|v| crate::dag_engine::engine::parse_bool_str(&v));
    let dir = get(ENV_STAGING_DIR).map(|v| v.trim().to_string());
    let (Some(true), Some(dir)) = (on, dir.filter(|d| !d.is_empty())) else {
        return Ok(None);
    };
    let path = PathBuf::from(&dir);
    let expected = "an absolute path of an existing directory, without '..'";
    if !path.is_absolute() || has_parent_dir_component(&path) {
        return Err(invalid(ENV_STAGING_DIR, &dir, expected));
    }
    match std::fs::canonicalize(&path) {
        Ok(p) if p.is_dir() => Ok(Some(p)),
        _ => Err(invalid(ENV_STAGING_DIR, &dir, expected)),
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
            hide_paths: hide_paths(get)?,
            staging_root: staging_root(get)?,
            max_request_bytes: parse_in(get, ENV_MAX_REQUEST_MB, 256usize, 1, 4095)? * MIB,
            max_response_bytes: parse_in(get, ENV_MAX_RESPONSE_MB, 256usize, 1, 4095)? * MIB,
            refuse_output: get(ENV_REFUSE_OUTPUT)
                .map(|v| {
                    v.split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
        })
    }
}

impl RemoteConfig {
    /// `None` when no URL is set. The URL only ever comes from the process
    /// environment, never from graph data.
    pub fn from_lookup(
        get: &impl Fn(&str) -> Option<String>,
        sub: &SubprocessConfig,
    ) -> Result<Option<Self>, ExecutorConfigError> {
        let val = |k: &str| {
            get(k)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        let Some(raw) = val(ENV_URL) else {
            return Ok(None);
        };
        // The value is never echoed: it may hold a secret, parsed or not.
        let bad = |expected: &str| {
            ExecutorConfigError(format!("{ENV_URL} is invalid; expected {expected}"))
        };
        let url = reqwest::Url::parse(&raw).map_err(|_| bad("an absolute URL"))?;
        // Credentials go in `COLMENA_PYTHON_EXECUTOR_AUTH`, never in the URL.
        let userinfo = !url.username().is_empty() || url.password().is_some();
        if userinfo || url.query().is_some() || url.fragment().is_some() {
            return Err(bad("no user, password, query or fragment"));
        }
        // Loopback as `python_executor serve` reads it (127.0.0.0/8, ::1), or `localhost`.
        let loopback = match url.host() {
            Some(url::Host::Domain(d)) => d == "localhost",
            Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
            Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
            None => false,
        };
        if url.scheme() != "https" && !(url.scheme() == "http" && loopback) {
            return Err(bad("https (plain http only for loopback)"));
        }
        let auth = match val(ENV_AUTH).as_deref() {
            Some("none") => RemoteAuthConfig::None,
            Some("bearer_file") => RemoteAuthConfig::BearerFile(
                val(ENV_TOKEN_FILE).map(PathBuf::from).ok_or_else(|| {
                    ExecutorConfigError(format!("{ENV_AUTH}=bearer_file needs {ENV_TOKEN_FILE}"))
                })?,
            ),
            Some("gcp_id_token") if url.scheme() == "https" => RemoteAuthConfig::GcpIdToken {
                audience: val(ENV_AUDIENCE).unwrap_or_else(|| url.origin().ascii_serialization()),
            },
            Some("gcp_id_token") => {
                return Err(invalid(
                    ENV_AUTH,
                    "gcp_id_token",
                    "an https URL for identity tokens",
                ))
            }
            Some(v) => return Err(invalid(ENV_AUTH, v, "none, bearer_file or gcp_id_token")),
            None => {
                return Err(ExecutorConfigError(format!(
                    "{ENV_AUTH} is required when {ENV_URL} is set"
                )))
            }
        };
        let max_wire_bytes = match val(ENV_MAX_WIRE_MB) {
            None => None,
            Some(_) => Some(parse_in(get, ENV_MAX_WIRE_MB, 0usize, 1, 4095)? * MIB),
        };
        Ok(Some(Self {
            url,
            auth,
            max_request_bytes: sub.max_request_bytes,
            max_response_bytes: sub.max_response_bytes,
            max_wire_bytes,
        }))
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
        let subprocess = SubprocessConfig::from_lookup(&get)?;
        Ok(Self {
            kind,
            modes,
            max_timeout,
            remote: RemoteConfig::from_lookup(&get, &subprocess)?,
            subprocess,
        })
    }
}

#[cfg(test)]
mod staging_tests {
    use super::*;

    fn staging_env<'a>(
        switch: Option<&'a str>,
        dir: Option<&'a str>,
    ) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| match k {
            ENV_LARGE_TABULAR => switch.map(String::from),
            ENV_STAGING_DIR => dir.map(String::from),
            _ => None,
        }
    }

    /// With the switch off (or unset) the staging directory is not even looked
    /// at: today's executor config, whatever the variable holds.
    #[test]
    fn staging_is_off_unless_the_switch_is_on_and_a_directory_is_set() {
        let real = std::env::temp_dir();
        let real = real.to_str().unwrap();
        for switch in [None, Some("off"), Some("0"), Some("maybe"), Some("")] {
            assert_eq!(staging_root(&staging_env(switch, Some(real))), Ok(None));
            assert_eq!(
                staging_root(&staging_env(switch, Some("../relative"))),
                Ok(None)
            );
        }
        assert_eq!(staging_root(&staging_env(Some("on"), None)), Ok(None));
        assert_eq!(staging_root(&staging_env(Some("on"), Some("  "))), Ok(None));
    }

    #[test]
    fn the_staging_directory_is_canonical_and_must_be_a_real_plain_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let canonical = tmp.path().canonicalize().unwrap();
        let ok = staging_root(&staging_env(Some("on"), tmp.path().to_str()));
        assert_eq!(ok, Ok(Some(canonical.clone())));
        let file = canonical.join("f");
        std::fs::write(&file, "x").unwrap();
        std::fs::create_dir(canonical.join("sub")).unwrap();
        let dotted = format!("{}/sub/..", canonical.display());
        for bad in [
            "relative/dir",
            "/does/not/exist",
            file.to_str().unwrap(),
            &dotted,
        ] {
            let got = staging_root(&staging_env(Some("on"), Some(bad)));
            assert!(got.is_err_and(|e| e.0.contains(ENV_STAGING_DIR)), "{bad}");
        }
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
    fn a_relative_hidden_path_is_an_error() {
        let e = cfg(&[(ENV_HIDE_PATHS, "/data:data")]).unwrap_err();
        assert!(e.0.contains(ENV_HIDE_PATHS), "{e}");
    }

    #[test]
    fn a_hidden_path_with_a_parent_dir_component_is_an_error() {
        let e = cfg(&[(ENV_HIDE_PATHS, "/home/../etc/app.key")]).unwrap_err();
        assert!(e.0.contains(ENV_HIDE_PATHS), "{e}");
        assert!(cfg(&[(ENV_HIDE_PATHS, "/data:/etc/../secret")]).is_err());
        assert!(cfg(&[(ENV_HIDE_PATHS, "/data")]).is_ok());
    }

    #[test]
    fn refused_output_literals_are_trimmed_and_blank_ones_dropped() {
        assert!(cfg(&[]).unwrap().subprocess.refuse_output.is_empty());
        let c = cfg(&[(ENV_REFUSE_OUTPUT, " ab- ,, cd_ , ")])
            .unwrap()
            .subprocess;
        assert_eq!(c.refuse_output, vec!["ab-".to_string(), "cd_".to_string()]);
        let blank = cfg(&[(ENV_REFUSE_OUTPUT, " , ,")]).unwrap().subprocess;
        assert!(blank.refuse_output.is_empty());
    }

    #[test]
    fn modes_names_are_the_env_values() {
        assert_eq!(ModesPolicy::Restricted.as_str(), "restricted");
        assert_eq!(ModesPolicy::All.as_str(), "all");
    }

    #[test]
    fn remote_requires_url_and_explicit_auth() {
        let only_kind = cfg(&[(ENV_EXECUTOR, "remote")]);
        assert!(only_kind.unwrap().remote.is_none()); // parsed; Dispatcher::build refuses it
        let e = cfg(&[(ENV_EXECUTOR, "remote"), (ENV_URL, "https://x.example")]).unwrap_err();
        assert!(e.0.contains(ENV_AUTH), "{e}");
        let r = cfg(&[
            (ENV_EXECUTOR, "remote"),
            (ENV_URL, "https://x.example/"),
            (ENV_AUTH, "gcp_id_token"),
        ])
        .unwrap()
        .remote
        .unwrap();
        let audience = "https://x.example".to_string();
        assert_eq!(r.auth, RemoteAuthConfig::GcpIdToken { audience });
    }

    #[test]
    fn plain_http_is_only_for_loopback_whatever_the_executor() {
        for kind in ["remote", "inprocess", "subprocess"] {
            let at = |url, auth| cfg(&[(ENV_EXECUTOR, kind), (ENV_URL, url), (ENV_AUTH, auth)]);
            assert!(at("http://10.0.0.5:8080", "none").is_err());
            assert!(at("http://127.0.0.1:8080", "none").is_ok());
            assert!(at("http://[::1]:8080", "none").is_ok());
            assert!(at("http://127.0.0.2:8080", "none").is_ok());
            assert!(at("http://localhost:8080", "none").is_ok());
            assert!(at("http://127.0.0.1:8080", "gcp_id_token").is_err());
            assert!(at("ftp://x.example", "none").is_err());
            assert!(at("not a url", "none").is_err());
        }
    }

    #[test]
    fn the_url_carries_no_credentials_and_is_never_echoed() {
        for (url, why) in [
            ("https://:hunter2@x.example/", "password"),
            ("https://u@x.example/", "password"),
            ("http://u:hunter2@10.0.0.5/", "password"),
            ("https://x.example/?k=hunter2", "query"),
            ("https://x.example/#hunter2", "fragment"),
            ("http://10.0.0.5/hunter2", "https"),
            ("hunter2 x.example", "absolute URL"),
        ] {
            let e = cfg(&[(ENV_URL, url), (ENV_AUTH, "none")]).unwrap_err().0;
            assert!(e.starts_with(ENV_URL) && e.contains(why), "{e}");
            assert!(!e.contains("hunter2") && !e.contains(".example"), "{e}");
        }
    }

    #[test]
    fn remote_auth_settings_and_wire_cap() {
        let base = [
            (ENV_EXECUTOR, "remote"),
            (ENV_URL, "https://x.example:8443/"),
        ];
        let with = |extra: &[(&str, &str)]| cfg(&[&base[..], extra].concat());
        let e = with(&[(ENV_AUTH, "bearer_file")]).unwrap_err();
        assert!(e.0.contains(ENV_TOKEN_FILE), "{e}");
        let r = with(&[(ENV_AUTH, "bearer_file"), (ENV_TOKEN_FILE, "/run/t")]);
        let r = r.unwrap().remote.unwrap();
        assert_eq!(
            r.auth,
            RemoteAuthConfig::BearerFile(PathBuf::from("/run/t"))
        );
        assert_eq!((r.max_request_bytes, r.max_wire_bytes), (256 * MIB, None));
        let r = with(&[(ENV_AUTH, "gcp_id_token")]).unwrap().remote.unwrap();
        let audience = "https://x.example:8443".to_string();
        assert_eq!(r.auth, RemoteAuthConfig::GcpIdToken { audience });
        let r = with(&[
            (ENV_AUTH, "gcp_id_token"),
            (ENV_AUDIENCE, "aud"),
            (ENV_MAX_WIRE_MB, "31"),
        ]);
        let r = r.unwrap().remote.unwrap();
        let audience = "aud".to_string();
        assert_eq!(r.auth, RemoteAuthConfig::GcpIdToken { audience });
        assert_eq!(r.max_wire_bytes, Some(31 * MIB));
        let e = with(&[(ENV_AUTH, "none"), (ENV_MAX_WIRE_MB, "0")]).unwrap_err();
        assert!(e.0.contains(ENV_MAX_WIRE_MB), "{e}");
        let e = with(&[(ENV_AUTH, "basic")]).unwrap_err();
        assert!(e.0.contains(ENV_AUTH), "{e}");
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
