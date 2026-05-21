use thiserror::Error;

/// Errors that can be returned by an [`OutputStorageRepository`](super::OutputStorageRepository)
/// implementation when persisting generated media (images, audio, etc.).
#[derive(Debug, Error)]
pub enum StorageError {
    /// The storage backend (object store, HTTP endpoint) could not be reached
    /// at all (DNS, TCP, TLS, transport-level failure).
    #[error("storage backend unavailable: {0}")]
    BackendUnavailable(String),

    /// The request was malformed before the backend was contacted (e.g. empty
    /// bytes, missing required metadata).
    #[error("invalid storage input: {0}")]
    InvalidInput(String),

    /// The upload step itself failed (non-2xx PUT to the signed URL, etc.).
    #[error("upload failed: {0}")]
    UploadFailed(String),

    /// The callback endpoint that should have issued a signed PUT URL returned
    /// a non-success status. `status` is the HTTP code (0 if the body could
    /// not be parsed as JSON).
    #[error("storage callback failed (status {status}): {body}")]
    CallbackFailed { status: u16, body: String },
}
