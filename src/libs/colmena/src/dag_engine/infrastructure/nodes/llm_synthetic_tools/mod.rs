//! Synthetic tools for the LLM node — tools that don't map to DAG nodes.

pub mod document_tools;
pub mod load_skill_tool;

pub use load_skill_tool::{
    build_load_skill_tool_definition, dispatch_load_skill, into_tool_result,
    LoadSkillDispatchResult, LOAD_SKILL_TOOL_NAME,
};

pub use document_tools::{
    build_all_document_tools, build_document_apply_patch_tool, build_document_create_tool,
    build_document_get_head_tool, build_document_list_my_artifacts_tool,
    build_document_list_versions_tool, build_document_read_tool, build_document_rollback_tool,
    dispatch_document_apply_patch, dispatch_document_create, dispatch_document_get_head,
    dispatch_document_list_my_artifacts, dispatch_document_list_versions, dispatch_document_read,
    dispatch_document_rollback, DocumentToolsContext, DOCUMENT_APPLY_PATCH_TOOL,
    DOCUMENT_CREATE_TOOL, DOCUMENT_GET_HEAD_TOOL, DOCUMENT_LIST_MY_ARTIFACTS_TOOL,
    DOCUMENT_LIST_VERSIONS_TOOL, DOCUMENT_READ_TOOL, DOCUMENT_ROLLBACK_TOOL,
};
