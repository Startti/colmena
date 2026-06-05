pub mod crdt_documents;
pub mod dag_engine;
pub mod documents;
pub mod gsheets;
pub mod llm;
pub mod skills;
pub mod storage;
pub mod web;

/// Print a debug/verbose message. No-op unless verbose mode is enabled via
/// `--verbose` CLI flag or `COLMENA_VERBOSE=1` env variable.
#[macro_export]
macro_rules! colmena_log {
    ($($arg:tt)*) => {
        if $crate::dag_engine::verbose::is_verbose() {
            println!($($arg)*);
        }
    };
}
#[cfg(feature = "node")]
pub mod node_bindings;
#[cfg(feature = "python")]
pub mod python_bindings;
pub mod shared;

pub use llm::*;
#[cfg(feature = "node")]
pub use node_bindings::*;
#[cfg(feature = "python")]
pub use python_bindings::*;
