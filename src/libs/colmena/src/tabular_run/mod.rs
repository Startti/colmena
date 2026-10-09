//! Running model-written Python over a PREPARED large tabular file (dark behind
//! `COLMENA_LARGE_TABULAR`; with the switch off nothing here runs).
//!
//! The preparation job (see [`crate::tabular_prepare`]) turned the file into
//! Parquet parts plus a manifest, tracked in the registry. This module takes
//! those prepared tables into the Python sandbox:
//!
//! - [`refusal`]: the typed, model-readable reasons a run is refused. There is
//!   no fallback that loads the original file.
//!
//! See docs/developer_guide/53_python_executors.md and
//! docs/developer_guide/54_tabular_prepare.md.

pub mod refusal;
