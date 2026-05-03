#[cfg(feature = "gcs")]
pub mod gcs_store;
pub mod local_fs_store;

#[cfg(feature = "gcs")]
pub use gcs_store::GcsArtifactStore;
pub use local_fs_store::LocalFsStore;
