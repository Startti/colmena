//! Running model-written Python over a PREPARED large tabular file (dark behind
//! `COLMENA_LARGE_TABULAR`; with the switch off nothing here runs).
//!
//! The preparation job (see [`crate::tabular_prepare`]) turned the file into
//! Parquet parts plus a manifest, tracked in the registry. This module takes
//! those prepared tables into the Python sandbox:
//!
//! - [`refusal`]: the typed, model-readable reasons a run is refused. There is
//!   no fallback that loads the original file.
//! - [`verify`]: is the prepared copy one the registry vouches for? Ownership,
//!   readiness, layout and manifest are checked before anything is staged.
//!
//! - [`stage`]: stream the parts into the call's data directory, bounded in
//!   memory and in bytes (Unix).
//!
//! - [`mounted`]: the seam an executor implements to run a call over the tables;
//!   `local` is the subprocess executor's, `remote_seam` the remote one's (refuses).
//!
//! - [`prelude`]: the Python the model's code finds (`tables`, a guarded `df`) and
//!   how that code is wrapped.
//!
//! - [`runtime`]: one call start to finish (ensure prepared, verify, run, report).
//!
//! - [`collect`]: read back what the code wrote to `/out`, treating all of it as
//!   hostile (Unix).
//!
//! - [`outputs`]: stream the kept outputs to storage, one chunk at a time.
//!
//! - [`wire`]: the `/v2/run` framing, shared by client and server, on any HTTP version.
//!
//! See docs/developer_guide/53_python_executors.md and
//! docs/developer_guide/54_tabular_prepare.md.

#[cfg(unix)]
pub mod collect;
#[cfg(target_os = "linux")]
pub mod local;
pub mod mounted;
#[cfg(unix)]
pub mod outputs;
pub mod prelude;
pub mod refusal;
pub mod remote_seam;
pub mod runtime;
#[cfg(unix)]
pub mod stage;
#[cfg(test)]
pub(crate) mod testkit;
pub mod verify;
pub mod wire;
