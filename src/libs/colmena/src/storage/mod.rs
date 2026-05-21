//! Storage module — persists generated output media (images, audio) produced
//! by DAG nodes. Defines a thin port ([`domain::OutputStorageRepository`]) and
//! ships two adapters: an in-memory [`infrastructure::LocalCacheStorageAdapter`]
//! for CLI / tests, and an [`infrastructure::HttpCallbackStorageAdapter`] for
//! production deployments that delegate storage to an external API.

pub mod domain;
pub mod infrastructure;
