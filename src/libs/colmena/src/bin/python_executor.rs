//! Isolated Python executor for Colmena. Linux only.

#[cfg(target_os = "linux")]
mod linux {
    use clap::{Parser, Subcommand};
    use colmena::dag_engine::infrastructure::python_exec::{child::JailSpec, zygote};
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
            #[arg(long = "hide")]
            hide: Vec<PathBuf>,
        },
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
