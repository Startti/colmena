//! The `describe_tool` synthetic tool — dispatches catalog lookups and produces
//! curated markdown for the LLM.

use crate::llm::domain::{LlmError, ToolCall, ToolResult};

pub const DESCRIBE_TOOL_NAME: &str = "describe_tool";

#[derive(Debug)]
pub struct DescribeToolDispatchResult {
    pub output: String,
    pub tool_name: String,
}

pub async fn dispatch_describe_tool(
    _tool_call: &ToolCall,
    _lookup: &[crate::dag_engine::domain::tool_configuration::ToolConfiguration],
) -> Result<DescribeToolDispatchResult, LlmError> {
    Ok(DescribeToolDispatchResult {
        output: String::new(),
        tool_name: String::new(),
    })
}

pub fn into_tool_result(call_id: &str, r: &DescribeToolDispatchResult) -> ToolResult {
    ToolResult {
        tool_call_id: call_id.to_string(),
        output: r.output.clone(),
        success: true,
        error: None,
    }
}
