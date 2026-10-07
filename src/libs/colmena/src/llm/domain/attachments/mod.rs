pub mod attachment_error;
pub mod attachment_registry;
pub mod auto_id;
pub mod conversation_attachment;
pub mod stream_resolver;
pub mod summary_generator;

pub use attachment_error::AttachmentError;
pub use attachment_registry::{
    AttachmentRegistry, StaleAttachmentQuery, UpsertAttachmentInput, UpsertOutcome,
};
pub use auto_id::generate_attachment_id;
pub use conversation_attachment::{AttachmentSource, ConversationAttachment};
pub use stream_resolver::{AttachmentResolveError, AttachmentStreamResolver};
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

    /// A large tabular file the HOST owns: the row's `storage_key` is the host's
    /// own key, not a copy the engine stored. Nothing in the engine may delete
    /// the object behind such a row (the row itself may be dropped), read it
    /// whole, send it to a provider or summarise it, whatever the row's size or
    /// mime says and whatever the large tabular switch says now.
    pub const HOST_STORAGE_REF: &str = "host_storage_ref";

    /// Whether `origin` marks a row the user supplied, by upload or by a host
    /// reference: such rows stay visible across providers.
    pub fn is_user_supplied(origin: Option<&str>) -> bool {
        matches!(origin, Some(USER_UPLOAD) | Some(HOST_STORAGE_REF))
    }

    /// Helper for tools that generate attachments. Produces
    /// `generated_by:<tool_name>` (e.g., `generated_by:image_generation`).
    pub fn generated_by(tool_name: &str) -> String {
        format!("generated_by:{}", tool_name)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn a_host_reference_is_distinct_from_an_upload_and_user_supplied() {
            assert_ne!(HOST_STORAGE_REF, USER_UPLOAD);
            assert_eq!(HOST_STORAGE_REF, "host_storage_ref");
            assert!(is_user_supplied(Some(USER_UPLOAD)));
            assert!(is_user_supplied(Some(HOST_STORAGE_REF)));
            assert!(!is_user_supplied(Some("generated_by:tts")));
            assert!(!is_user_supplied(None));
        }

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
