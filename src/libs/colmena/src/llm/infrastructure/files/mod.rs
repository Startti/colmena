//! Adapters para Files APIs de proveedores LLM y cache persistido.

pub mod postgres_file_cache;
pub mod signed_url_downloader;

pub use postgres_file_cache::PostgresFileCache;
pub use signed_url_downloader::SignedUrlDownloader;
