//! Domain layer for the storage module. Contains the [`OutputStorageRepository`]
//! port plus its value objects ([`StoreRequest`], [`StoredOutput`]) and the
//! [`StorageError`] enum. No infrastructure dependencies.

pub mod output_storage_repository;
pub mod storage_error;

pub use output_storage_repository::{
    OutputStorageRepository, StoreRequest, StoredBytes, StoredOutput,
};
pub use storage_error::StorageError;

#[cfg(test)]
pub use output_storage_repository::MockOutputStorageRepository;
