//! Isolated Python executor for Colmena. Linux only.

#[cfg(target_os = "linux")]
mod linux {
    use clap::{Parser, Subcommand};
    use colmena::dag_engine::infrastructure::python_exec::child::{JailSpec, EXIT_NOT_READY};
    use colmena::dag_engine::infrastructure::python_exec::config::ExecutorConfig;
    use colmena::dag_engine::infrastructure::python_exec::{selftest, server, zygote};
    use std::path::PathBuf;

    #[derive(Parser)]
    #[command(
        name = "python_executor",
        about = "Isolated Python executor for Colmena"
    )]
    struct Cli {
        #[command(subcommand)]
        cmd: Cmd,
    }

    #[derive(Subcommand)]
    enum Cmd {
        /// Internal: warm template started by the subprocess executor.
        Zygote {
            #[arg(long)]
            socket: PathBuf,
            #[arg(long, default_value_t = 20000)]
            uid_base: u32,
            #[arg(long, default_value_t = 64)]
            tmp_mb: u64,
            #[arg(long = "hide", value_parser = absolute)]
            hide: Vec<PathBuf>,
        },
        /// Proves each layer of the process jail in a throwaway child. Prints
        /// one JSON object per check and exits 0 only when every one held.
        SelfTest {
            #[arg(long, default_value_t = 20000)]
            uid_base: u32,
            #[arg(long, default_value_t = 64)]
            tmp_mb: u64,
            #[arg(long = "hide", value_parser = absolute)]
            hide: Vec<PathBuf>,
        },
        /// HTTP front for remote callers: `POST /v1/run`, `GET /healthz` and
        /// `GET /readyz`. The executor takes the `COLMENA_PYTHON_EXECUTOR_*`
        /// settings a host would.
        Serve {
            #[arg(long, default_value = "127.0.0.1:8080")]
            listen: std::net::SocketAddr,
            /// Every request must carry `Authorization: Bearer <file content>`;
            /// required unless --listen is a loopback address.
            #[arg(long)]
            token_file: Option<PathBuf>,
        },
    }

    /// A relative path would be resolved against the working directory, and a
    /// `..` component would let a path walk back out of what it looks like it
    /// names lexically (`/home/../etc/app.key`), which the self-test's
    /// nested-path exclusion reasons about by prefix, not by resolving it.
    fn absolute(s: &str) -> Result<PathBuf, String> {
        let path = PathBuf::from(s);
        if !path.is_absolute() {
            return Err("expected an absolute path".into());
        }
        if path
            .components()
            .any(|c| c == std::path::Component::ParentDir)
        {
            return Err("must not contain a '..' component".into());
        }
        Ok(path)
    }

    pub fn main() -> i32 {
        match Cli::parse().cmd {
            Cmd::Zygote {
                socket,
                uid_base,
                tmp_mb,
                hide,
            } => zygote::run(zygote::ZygoteArgs {
                socket,
                jail: JailSpec {
                    uid_base,
                    tmp_mb,
                    hide_paths: hide,
                },
            }),
            Cmd::SelfTest {
                uid_base,
                tmp_mb,
                hide,
            } => {
                let spec = JailSpec {
                    uid_base,
                    tmp_mb,
                    hide_paths: hide,
                };
                let (code, checks) = match selftest::run(&spec) {
                    Ok(checks) => (0, checks),
                    Err(checks) => (EXIT_NOT_READY, checks),
                };
                for check in checks {
                    println!("{}", serde_json::to_string(&check).unwrap_or_default());
                }
                code
            }
            Cmd::Serve { listen, token_file } => {
                let filter = tracing_subscriber::EnvFilter::try_from_default_env();
                let log = tracing_subscriber::fmt().with_ansi(false);
                let _ = log
                    .with_env_filter(filter.unwrap_or_else(|_| "info".into()))
                    .try_init();
                let cfg = match ExecutorConfig::from_env() {
                    Ok(cfg) => cfg,
                    Err(e) => {
                        eprintln!("python_executor: {e}");
                        return 2;
                    }
                };
                server::run(server::ServeArgs {
                    listen,
                    subprocess: cfg.subprocess,
                    max_timeout: cfg.max_timeout,
                    token_file,
                })
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn a_plain_absolute_path_is_accepted() {
            assert_eq!(absolute("/data").unwrap(), PathBuf::from("/data"));
        }

        #[test]
        fn a_relative_path_is_rejected() {
            assert!(absolute("data").is_err());
        }

        #[test]
        fn a_path_with_a_parent_dir_component_is_rejected() {
            assert!(absolute("/home/../etc/app.key").is_err());
            assert!(absolute("../etc/app.key").is_err());
            assert!(absolute("/etc/app.key/..").is_err());
        }
    }
}

fn main() {
    #[cfg(target_os = "linux")]
    std::process::exit(linux::main());
    #[cfg(not(target_os = "linux"))]
    {
        eprintln!("python_executor: unsupported platform (Linux only)");
        std::process::exit(2);
    }
}
