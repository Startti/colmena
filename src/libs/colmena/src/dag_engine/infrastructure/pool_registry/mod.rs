//! Shared Postgres connection-pool registry.
//!
//! Single source of truth for all `PgPool` instances in the engine. One pool per
//! unique (normalized) `connection_url`, reused across jobs and consumers.
//! See `docs/superpowers/specs/2026-04-20-connection-pool-management-design.md`.

mod config;
mod error;
mod metrics;
mod registry;
mod url_key;

pub use config::{ConfigError, PoolConfig};
pub use error::RegistryError;
pub use metrics::{PoolMetrics, RegistryMetrics};
pub use registry::PgPoolRegistry;
#[allow(unused_imports)]
pub(crate) use url_key::UrlKey;
