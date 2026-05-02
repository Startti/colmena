//! Adapters para Files APIs de proveedores LLM y cache persistido.

pub mod anthropic_files_api;
pub mod gemini_files_api;
pub mod openai_files_api;
pub mod postgres_file_cache;
pub mod signed_url_downloader;

pub use anthropic_files_api::AnthropicFilesApiAdapter;
pub use gemini_files_api::GeminiFilesApiAdapter;
pub use openai_files_api::OpenAiFilesApiAdapter;
pub use postgres_file_cache::PostgresFileCache;
pub use signed_url_downloader::SignedUrlDownloader;
