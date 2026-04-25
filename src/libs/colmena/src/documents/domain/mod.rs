pub mod artifact;
pub mod error;
pub mod ids;
pub mod ir;
pub mod patch;
pub mod ports;

pub use artifact::{
    ArtifactMeta, ArtifactSummary, AssignedIds, OpOutcome, PatchApplied, PatchSummary, VersionData,
};
pub use error::{ConflictDetail, DocumentError, IndexError, RenderError, StorageError};
pub use ids::{ArtifactId, ArtifactKind, SessionId, VersionId};
pub use patch::{Patch, PatchOp, PatchSource};
pub use ports::{ArtifactStore, IRRenderer, IRValidator, IdGenerator, SessionArtifactIndex};
