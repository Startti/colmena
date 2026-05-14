//! Synthetic tools for the LLM node — tools that don't map to DAG nodes.

pub mod describe_tool;
pub mod document_tools;
pub mod lazy_tools_catalog;
pub mod load_attachment_tool;
pub mod load_skill_tool;

pub use describe_tool::{
    dispatch_describe_tool, into_tool_result as describe_tool_into_tool_result,
    DescribeToolDispatchResult, DESCRIBE_TOOL_NAME,
};

pub use document_tools::{
    build_all_document_tools, build_document_apply_patch_tool, build_document_create_tool,
    build_document_get_head_tool, build_document_list_my_artifacts_tool,
    build_document_list_versions_tool, build_document_read_tool, build_document_rollback_tool,
    dispatch_document_apply_patch, dispatch_document_create, dispatch_document_get_head,
    dispatch_document_list_my_artifacts, dispatch_document_list_versions, dispatch_document_read,
    dispatch_document_rollback, DocumentToolsContext, DOCUMENTS_SYSTEM_PRELUDE,
    DOCUMENT_APPLY_PATCH_TOOL, DOCUMENT_CREATE_TOOL, DOCUMENT_GET_HEAD_TOOL,
    DOCUMENT_LIST_MY_ARTIFACTS_TOOL, DOCUMENT_LIST_VERSIONS_TOOL, DOCUMENT_READ_TOOL,
    DOCUMENT_ROLLBACK_TOOL,
};

pub use lazy_tools_catalog::{
    build_describe_tool_definition, reconstruct_discovered_set, summary_for_catalog, CatalogEntry,
};

pub use load_attachment_tool::{
    build_load_attachment_tool_definition, dispatch_load_attachment, ATTACHMENTS_SYSTEM_PRELUDE,
    LOAD_ATTACHMENT_TOOL_NAME,
};

pub use load_skill_tool::{
    build_load_skill_tool_definition, dispatch_load_skill, into_tool_result,
    LoadSkillDispatchResult, LOAD_SKILL_TOOL_NAME,
};
