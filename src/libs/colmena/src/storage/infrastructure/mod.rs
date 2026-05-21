//! Infrastructure adapters that implement
//! [`OutputStorageRepository`](crate::storage::domain::OutputStorageRepository).

pub mod http_callback_adapter;
pub mod local_cache_adapter;
pub mod local_http_adapter;

pub use http_callback_adapter::HttpCallbackStorageAdapter;
pub use local_cache_adapter::LocalCacheStorageAdapter;
pub use local_http_adapter::LocalHttpStorageAdapter;
