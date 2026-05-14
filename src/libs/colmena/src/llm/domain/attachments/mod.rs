pub mod attachment_error;
pub mod attachment_registry;
pub mod auto_id;
pub mod conversation_attachment;
pub mod summary_generator;

pub use attachment_error::AttachmentError;
pub use attachment_registry::{AttachmentRegistry, UpsertAttachmentInput};
pub use auto_id::generate_attachment_id;
pub use conversation_attachment::{AttachmentSource, ConversationAttachment};
pub use summary_generator::{
    AttachmentSummaryGenerator, SummaryConfig, SummaryError, SummaryInput, SummaryOutcome,
    SummarySource,
};
