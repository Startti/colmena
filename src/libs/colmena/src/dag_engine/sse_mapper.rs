use std::collections::HashMap;

use serde_json::{json, Value};
use uuid::Uuid;

use crate::dag_engine::domain::events::DagExecutionEvent;

/// Stateful mapper from `DagExecutionEvent` to the SSE Data Stream Protocol JSON parts.
///
/// Mirrors exactly what `colmena run` emits line-by-line to stdout. Any consumer
/// (HTTP SSE handler, Redis worker, tests) can use this instead of hand-rolling
/// the same match block and diverging over time.
pub struct SseMapper {
    text_block_ids: HashMap<String, String>,
    node_types: HashMap<String, String>,
    total_prompt_tokens: u64,
    total_completion_tokens: u64,
    total_thinking_tokens: u64,
    total_cache_read_tokens: u64,
    total_cache_write_tokens: u64,
}

impl Default for SseMapper {
    fn default() -> Self {
        Self::new()
    }
}

impl SseMapper {
    pub fn new() -> Self {
        Self {
            text_block_ids: HashMap::new(),
            node_types: HashMap::new(),
            total_prompt_tokens: 0,
            total_completion_tokens: 0,
            total_thinking_tokens: 0,
            total_cache_read_tokens: 0,
            total_cache_write_tokens: 0,
        }
    }

    /// Convert one `DagExecutionEvent` into the ordered list of JSON protocol parts
    /// that should be emitted for that event. Returns 0..N values.
    pub fn map(&mut self, event: &DagExecutionEvent) -> Vec<Value> {
        let mut parts: Vec<Value> = Vec::new();

        // Phase 1: state management — open/close text blocks, accumulate tokens
        match event {
            DagExecutionEvent::LlmToken { node_id, .. } => {
                if !self.text_block_ids.contains_key(node_id) {
                    let part_id = format!("txt_{}", Uuid::new_v4());
                    parts.push(json!({ "type": "text-start", "id": part_id }));
                    self.text_block_ids.insert(node_id.clone(), part_id);
                }
            }
            DagExecutionEvent::NodeFinish { node_id, .. }
            | DagExecutionEvent::SubgraphNodeFinish { node_id, .. } => {
                if let Some(part_id) = self.text_block_ids.remove(node_id) {
                    parts.push(json!({ "type": "text-end", "id": part_id }));
                }
            }
            DagExecutionEvent::LlmUsage {
                prompt_tokens,
                completion_tokens,
                thinking_tokens,
                cache_read_tokens,
                cache_write_tokens,
                ..
            } => {
                self.total_prompt_tokens += *prompt_tokens as u64;
                self.total_completion_tokens += *completion_tokens as u64;
                self.total_thinking_tokens += thinking_tokens.unwrap_or(0) as u64;
                self.total_cache_read_tokens += cache_read_tokens.unwrap_or(0) as u64;
                self.total_cache_write_tokens += cache_write_tokens.unwrap_or(0) as u64;
            }
            DagExecutionEvent::SubgraphWrapped { inner } => match inner.as_ref() {
                DagExecutionEvent::LlmToken { node_id, .. }
                | DagExecutionEvent::ThinkingToken { node_id, .. } => {
                    if !self.text_block_ids.contains_key(node_id) {
                        let part_id = format!("txt_{}", Uuid::new_v4());
                        parts.push(json!({ "type": "subgraph-text-start", "id": part_id }));
                        self.text_block_ids.insert(node_id.clone(), part_id);
                    }
                }
                DagExecutionEvent::NodeFinish { node_id, .. }
                | DagExecutionEvent::SubgraphNodeFinish { node_id, .. } => {
                    if let Some(part_id) = self.text_block_ids.remove(node_id) {
                        parts.push(json!({ "type": "subgraph-text-end", "id": part_id }));
                    }
                }
                DagExecutionEvent::LlmUsage {
                    prompt_tokens,
                    completion_tokens,
                    thinking_tokens,
                    cache_read_tokens,
                    cache_write_tokens,
                    ..
                } => {
                    self.total_prompt_tokens += *prompt_tokens as u64;
                    self.total_completion_tokens += *completion_tokens as u64;
                    self.total_thinking_tokens += thinking_tokens.unwrap_or(0) as u64;
                    self.total_cache_read_tokens += cache_read_tokens.unwrap_or(0) as u64;
                    self.total_cache_write_tokens += cache_write_tokens.unwrap_or(0) as u64;
                }
                _ => {}
            },
            _ => {}
        }

        // Phase 2: map event → protocol JSON
        let protocol: Option<Value> = match event {
            DagExecutionEvent::NodeStart { node_id, node_type, config, inputs } => {
                self.node_types.insert(node_id.clone(), node_type.clone());
                Some(json!({
                    "type": "node-start",
                    "node_id": node_id,
                    "node_type": node_type,
                    "config": config,
                    "inputs": Self::clean_inputs(inputs)
                }))
            }
            DagExecutionEvent::TurnStart { .. } => None,
            DagExecutionEvent::NodeFinish { node_id, output } => {
                let ntype = self.node_types.get(node_id).cloned().unwrap_or_default();
                Some(json!({
                    "type": "node-end",
                    "node_id": node_id,
                    "node_type": ntype,
                    "output": output
                }))
            }
            DagExecutionEvent::SubgraphNodeFinish { node_id, output } => Some(json!({
                "type": "node-end",
                "node_id": node_id,
                "node_type": "subgraph",
                "output": output
            })),
            DagExecutionEvent::LlmToken { node_id, token } => {
                let part_id = self
                    .text_block_ids
                    .get(node_id)
                    .cloned()
                    .unwrap_or_else(|| node_id.clone());
                Some(json!({ "type": "text-delta", "id": part_id, "delta": token }))
            }
            // ThinkingToken is emitted by the orchestrator node when internal planner/critic/reactor
            // LLMs stream tokens. Distinct from user-facing LlmToken.
            DagExecutionEvent::ThinkingToken { node_id, token } => Some(json!({
                "type": "thinking-delta",
                "node_id": node_id,
                "delta": token
            })),
            DagExecutionEvent::ReasoningStart { id, .. } => Some(json!({
                "type": "reasoning-start",
                "id": id
            })),
            DagExecutionEvent::ReasoningDelta { id, token, .. } => Some(json!({
                "type": "reasoning-delta",
                "id": id,
                "delta": token
            })),
            DagExecutionEvent::ReasoningEnd { id, .. } => Some(json!({
                "type": "reasoning-end",
                "id": id
            })),
            DagExecutionEvent::LlmUsage { .. } => None,
            DagExecutionEvent::GraphUsageSummary { entries } => Some(json!({
                "type": "usage-summary",
                "nodes": entries
            })),
            DagExecutionEvent::LlmToolCall { tool_id, args_chunk, .. } => Some(json!({
                "type": "tool-input-delta",
                "toolCallId": tool_id,
                "inputTextDelta": args_chunk
            })),
            DagExecutionEvent::LlmToolCallStart { tool_id, tool_name, tool_args, .. } => {
                let input = serde_json::from_str::<Value>(tool_args)
                    .unwrap_or_else(|_| Value::String(tool_args.clone()));
                Some(json!({
                    "type": "tool-input-available",
                    "toolCallId": tool_id,
                    "toolName": tool_name,
                    "input": input
                }))
            }
            DagExecutionEvent::LlmToolCallFinish { tool_id, output, .. } => {
                let out = serde_json::from_str::<Value>(output)
                    .unwrap_or_else(|_| Value::String(output.clone()));
                Some(json!({
                    "type": "tool-output-available",
                    "toolCallId": tool_id,
                    "output": out
                }))
            }
            DagExecutionEvent::GraphFinish { output } => {
                // Close any still-open text blocks before finish
                for part_id in self.text_block_ids.values() {
                    parts.push(json!({ "type": "text-end", "id": part_id }));
                }
                self.text_block_ids.clear();

                let finish_reason = output
                    .get("__colmena_status")
                    .or_else(|| {
                        output
                            .get("extra_info")
                            .and_then(|e| e.get("__colmena_status"))
                    })
                    .and_then(|s| s.as_str())
                    .map(|s| if s == "SUSPENDED" { "suspended" } else { "stop" })
                    .unwrap_or("stop");

                let mut usage_obj = json!({
                    "promptTokens": self.total_prompt_tokens,
                    "completionTokens": self.total_completion_tokens,
                    "totalTokens": self.total_prompt_tokens + self.total_completion_tokens + self.total_thinking_tokens
                });
                if self.total_thinking_tokens > 0 {
                    usage_obj["thinkingTokens"] = json!(self.total_thinking_tokens);
                }
                if self.total_cache_read_tokens > 0 {
                    usage_obj["cacheReadTokens"] = json!(self.total_cache_read_tokens);
                }
                if self.total_cache_write_tokens > 0 {
                    usage_obj["cacheWriteTokens"] = json!(self.total_cache_write_tokens);
                }

                Some(json!({
                    "type": "finish",
                    "finishReason": finish_reason,
                    "usage": usage_obj,
                    "output": output
                }))
            }
            DagExecutionEvent::LlmMessageStart { .. } => None,
            DagExecutionEvent::LlmMessageFinish { .. } => None,
            DagExecutionEvent::Error { message } => Some(json!({
                "type": "error",
                "errorText": message
            })),
            DagExecutionEvent::SkillLoaded {
                node_id,
                tool_id,
                skill_name,
                reference,
                source,
                size_bytes,
            } => Some(json!({
                "type": "skill-loaded",
                "nodeId": node_id,
                "toolCallId": tool_id,
                "skillName": skill_name,
                "reference": reference,
                "source": source,
                "sizeBytes": size_bytes,
            })),
            DagExecutionEvent::SubgraphWrapped { inner } => match inner.as_ref() {
                DagExecutionEvent::NodeStart { node_id, node_type, inputs, config } => {
                    self.node_types.insert(node_id.clone(), node_type.clone());
                    Some(json!({
                        "type": "subgraph-node-start",
                        "node_id": node_id,
                        "node_type": node_type,
                        "config": config,
                        "inputs": Self::clean_inputs(inputs)
                    }))
                }
                DagExecutionEvent::NodeFinish { node_id, output } => {
                    let ntype = self.node_types.get(node_id).cloned().unwrap_or_default();
                    Some(json!({
                        "type": "subgraph-node-end",
                        "node_id": node_id,
                        "node_type": ntype,
                        "output": output
                    }))
                }
                DagExecutionEvent::SubgraphNodeFinish { node_id, output } => Some(json!({
                    "type": "subgraph-node-end",
                    "node_id": node_id,
                    "node_type": "subgraph",
                    "output": output
                })),
                DagExecutionEvent::LlmToken { node_id, token } => {
                    let part_id = self
                        .text_block_ids
                        .get(node_id)
                        .cloned()
                        .unwrap_or_else(|| node_id.clone());
                    Some(json!({ "type": "subgraph-text-delta", "id": part_id, "delta": token }))
                }
                DagExecutionEvent::ThinkingToken { node_id, token } => {
                    let part_id = self
                        .text_block_ids
                        .get(node_id)
                        .cloned()
                        .unwrap_or_else(|| node_id.clone());
                    Some(json!({ "type": "subgraph-text-delta", "id": part_id, "delta": token }))
                }
                DagExecutionEvent::LlmToolCall { tool_id, args_chunk, .. } => Some(json!({
                    "type": "subgraph-tool-input-delta",
                    "toolCallId": tool_id,
                    "inputTextDelta": args_chunk
                })),
                DagExecutionEvent::LlmToolCallStart { tool_id, tool_name, tool_args, .. } => {
                    let input = serde_json::from_str::<Value>(tool_args)
                        .unwrap_or_else(|_| Value::String(tool_args.clone()));
                    Some(json!({
                        "type": "subgraph-tool-input-available",
                        "toolCallId": tool_id,
                        "toolName": tool_name,
                        "input": input
                    }))
                }
                DagExecutionEvent::LlmToolCallFinish { tool_id, output, .. } => {
                    let out = serde_json::from_str::<Value>(output)
                        .unwrap_or_else(|_| Value::String(output.clone()));
                    Some(json!({
                        "type": "subgraph-tool-output-available",
                        "toolCallId": tool_id,
                        "output": out
                    }))
                }
                DagExecutionEvent::ReasoningStart { id, .. } => Some(json!({
                    "type": "subgraph-reasoning-start",
                    "id": id
                })),
                DagExecutionEvent::ReasoningDelta { id, token, .. } => Some(json!({
                    "type": "subgraph-reasoning-delta",
                    "id": id,
                    "delta": token
                })),
                DagExecutionEvent::ReasoningEnd { id, .. } => Some(json!({
                    "type": "subgraph-reasoning-end",
                    "id": id
                })),
                DagExecutionEvent::SkillLoaded {
                    node_id,
                    tool_id,
                    skill_name,
                    reference,
                    source,
                    size_bytes,
                } => Some(json!({
                    "type": "subgraph-skill-loaded",
                    "nodeId": node_id,
                    "toolCallId": tool_id,
                    "skillName": skill_name,
                    "reference": reference,
                    "source": source,
                    "sizeBytes": size_bytes,
                })),
                DagExecutionEvent::GraphUsageSummary { entries } => Some(json!({
                    "type": "subgraph-usage-summary",
                    "nodes": entries
                })),
                DagExecutionEvent::Error { message } => Some(json!({
                    "type": "subgraph-error",
                    "errorText": message
                })),
                _ => None,
            },
        };

        if let Some(v) = protocol {
            parts.push(v);
        }
        parts
    }

    fn clean_inputs(inputs: &Value) -> Value {
        if let Some(obj) = inputs.as_object() {
            Value::Object(
                obj.iter()
                    .filter(|(k, _)| !k.starts_with("__") && k.as_str() != "session_id")
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            )
        } else {
            inputs.clone()
        }
    }
}
