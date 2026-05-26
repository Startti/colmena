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

/// Plan A: well-known values for `ConversationAttachment::origin` /
/// `UpsertAttachmentInput::origin`. Use these constants instead of hardcoding
/// strings at call sites so the catalog of origins stays grep-able and
/// drift-free across the tools that auto-register attachments.
pub mod origin {
    /// File uploaded by the user (inline data or signed URL).
    pub const USER_UPLOAD: &str = "user_upload";

    /// Helper for tools that generate attachments. Produces
    /// `generated_by:<tool_name>` (e.g., `generated_by:image_generation`).
    pub fn generated_by(tool_name: &str) -> String {
        format!("generated_by:{}", tool_name)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn user_upload_constant_value() {
            assert_eq!(USER_UPLOAD, "user_upload");
        }

        #[test]
        fn generated_by_formats_tool_name() {
            assert_eq!(
                generated_by("image_generation"),
                "generated_by:image_generation"
            );
            assert_eq!(generated_by("image_edit"), "generated_by:image_edit");
            assert_eq!(generated_by("tts"), "generated_by:tts");
        }
    }
}
