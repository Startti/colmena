//! Adapters para Files APIs de proveedores LLM y cache persistido.

pub mod postgres_file_cache;
pub use postgres_file_cache::PostgresFileCache;
