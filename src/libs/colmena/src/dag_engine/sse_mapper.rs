use std::collections::{HashMap, HashSet};

use serde_json::{json, Value};
use uuid::Uuid;

use crate::dag_engine::domain::events::DagExecutionEvent;

/// Stateful mapper from `DagExecutionEvent` to the SSE Data Stream Protocol JSON parts.
///
/// Mirrors exactly what `colmena run` emits line-by-line to stdout. Any consumer
/// (HTTP SSE handler, Redis worker, tests) can use this instead of hand-rolling
/// the same match block and diverging over time.
pub struct SseMapper {
    /// Open text blocks, keyed by the event's **lineage `path`** — never by
    /// `node_id` alone.
    ///
    /// One mapper serves every nesting level of a run, so `node_id` is not
    /// unique across it: two sub-agents in different branches can carry the same
    /// node id (the same `child_graph_inline` instantiated twice, a `for_each`
    /// fanning out over `subgraph`). Keyed by node id, the second open
    /// overwrites the first's uuid, the first `NodeFinish` closes the second's
    /// block, and the second's block never closes at all. `path` is unique per
    /// branch, so the pairing holds.
    text_block_ids: HashMap<String, String>,
    node_types: HashMap<String, String>,
    seen_top_tool_ids: HashSet<String>,
    seen_sub_tool_ids: HashSet<String>,
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
            seen_top_tool_ids: HashSet::new(),
            seen_sub_tool_ids: HashSet::new(),
            total_prompt_tokens: 0,
            total_completion_tokens: 0,
            total_thinking_tokens: 0,
            total_cache_read_tokens: 0,
            total_cache_write_tokens: 0,
        }
    }

    /// Follow a chain of `SubgraphWrapped` events down to the base (non-wrapped)
    /// event. Tolerates both the flat representation (`inner` is already a base
    /// event) and legacy nested wrappers (`SubgraphWrapped { SubgraphWrapped {…} }`).
    fn deep_base(event: &DagExecutionEvent) -> &DagExecutionEvent {
        let mut cur = event;
        while let DagExecutionEvent::SubgraphWrapped { inner, .. } = cur {
            cur = inner.as_ref();
        }
        cur
    }

    /// Compute the nesting `level` and lineage `path` for an event. Level-0
    /// (non-wrapped) events return `(0, node_id)`. Wrapped events accumulate the
    /// `depth` of every layer (so a legacy double-nested wrapper yields the same
    /// level as a flat wrapper of the same depth) and take the outermost
    /// non-empty `path`, falling back to the base event's `node_id`.
    fn level_and_path(event: &DagExecutionEvent) -> (u32, String) {
        let mut level = 0u32;
        let mut path = String::new();
        let mut cur = event;
        while let DagExecutionEvent::SubgraphWrapped {
            inner,
            depth,
            path: p,
        } = cur
        {
            level += (*depth).max(1);
            if path.is_empty() && !p.is_empty() {
                path = p.clone();
            }
            cur = inner.as_ref();
        }
        if path.is_empty() {
            if let Some(nid) = cur.node_id() {
                path = nid.to_string();
            }
        }
        (level, path)
    }

    /// Run-level token totals, in the shape every stream terminator emits.
    ///
    /// Field names are the ones consumers already bind to — none were renamed.
    /// Two things changed in their meaning:
    ///
    /// * `promptTokens` is now *fresh* input on every provider. The adapters
    ///   normalize away the API-level disagreement (Anthropic reports input net
    ///   of cache; OpenAI and Gemini fold cache into it), so this number is
    ///   comparable and summable even when one graph mixes providers.
    /// * `totalTokens` includes the cache columns. It previously omitted them,
    ///   which understated a cached Anthropic turn by roughly 80%.
    ///
    /// `cacheReadTokens` and `cacheWriteTokens` are always present, including as
    /// `0` — an absent field used to be ambiguous between "no cache hit" and
    /// "this provider does not report it". They stay two separate fields because
    /// their rates differ by more than 10x, so a combined figure could not be
    /// billed. `thinkingTokens` keeps its existing `> 0` gate.
    fn usage_snapshot(&self) -> Value {
        let mut usage_obj = json!({
            "promptTokens": self.total_prompt_tokens,
            "completionTokens": self.total_completion_tokens,
            "cacheReadTokens": self.total_cache_read_tokens,
            "cacheWriteTokens": self.total_cache_write_tokens,
            "totalTokens": self.total_prompt_tokens
                + self.total_completion_tokens
                + self.total_thinking_tokens
                + self.total_cache_read_tokens
                + self.total_cache_write_tokens
        });
        if self.total_thinking_tokens > 0 {
            usage_obj["thinkingTokens"] = json!(self.total_thinking_tokens);
        }
        usage_obj
    }

    /// Convert one `DagExecutionEvent` into the ordered list of JSON protocol parts
    /// that should be emitted for that event. Returns 0..N values.
    ///
    /// Every emitted frame carries two additive fields: `level` (nesting depth,
    /// `0` = top agent) and `path` (lineage `parent>…>node`). See
    /// `SPEC_NESTED_VISIBILITY_SSE_FIELDS.md`.
    pub fn map(&mut self, event: &DagExecutionEvent) -> Vec<Value> {
        let mut parts: Vec<Value> = Vec::new();

        // Computed up front rather than at the tagging step below: `path` is the
        // key for `text_block_ids`, and both phases need it.
        let (level, path) = Self::level_and_path(event);

        // Phase 1: state management — open/close text blocks, accumulate tokens
        match event {
            DagExecutionEvent::LlmToken { .. } if !self.text_block_ids.contains_key(&path) => {
                let part_id = format!("txt_{}", Uuid::new_v4());
                parts.push(json!({ "type": "text-start", "id": part_id }));
                self.text_block_ids.insert(path.clone(), part_id);
            }
            DagExecutionEvent::NodeFinish { .. } | DagExecutionEvent::SubgraphNodeFinish { .. } => {
                if let Some(part_id) = self.text_block_ids.remove(&path) {
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
            DagExecutionEvent::LlmToolCall {
                tool_id, tool_name, ..
            } if self.seen_top_tool_ids.insert(tool_id.clone()) => {
                parts.push(json!({
                    "type": "tool-input-start",
                    "toolCallId": tool_id,
                    "toolName": tool_name
                }));
            }
            DagExecutionEvent::SubgraphWrapped { inner, .. } => match Self::deep_base(inner) {
                // ThinkingToken deliberately does NOT open a text block: it maps
                // to `thinking-delta`, which carries no block id — the
                // same shape as top-level `thinking-delta`. It used to be folded
                // in here, which opened a `subgraph-text-start` whose deltas then
                // rendered an agent's internal reasoning as its answer.
                DagExecutionEvent::LlmToken { .. } if !self.text_block_ids.contains_key(&path) => {
                    let part_id = format!("txt_{}", Uuid::new_v4());
                    parts.push(json!({ "type": "subgraph-text-start", "id": part_id }));
                    self.text_block_ids.insert(path.clone(), part_id);
                }
                DagExecutionEvent::NodeFinish { .. }
                | DagExecutionEvent::SubgraphNodeFinish { .. } => {
                    if let Some(part_id) = self.text_block_ids.remove(&path) {
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
                DagExecutionEvent::LlmToolCall {
                    tool_id, tool_name, ..
                } if self.seen_sub_tool_ids.insert(tool_id.clone()) => {
                    parts.push(json!({
                        "type": "subgraph-tool-input-start",
                        "toolCallId": tool_id,
                        "toolName": tool_name
                    }));
                }
                _ => {}
            },
            _ => {}
        }

        // Phase 2: map event → protocol JSON
        let protocol: Option<Value> = match event {
            DagExecutionEvent::NodeStart {
                node_id,
                node_type,
                config,
                inputs,
            } => {
                self.node_types.insert(node_id.clone(), node_type.clone());
                Some(json!({
                    "type": "node-start",
                    "node_id": node_id,
                    "node_type": node_type,
                    "config": config,
                    "inputs": Self::clean_inputs(inputs)
                }))
            }
            DagExecutionEvent::NodeSkipped { node_id, reason } => Some(json!({
                "type": "node-skipped",
                "node_id": node_id,
                "reason": reason
            })),
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
            DagExecutionEvent::LlmToken { token, .. } => {
                let part_id = self
                    .text_block_ids
                    .get(&path)
                    .cloned()
                    .unwrap_or_else(|| path.clone());
                Some(json!({ "type": "text-delta", "id": part_id, "delta": token }))
            }
            // ThinkingToken is emitted by the orchestrator node when internal planner/critic/reactor
            // LLMs stream tokens. Distinct from user-facing LlmToken.
            DagExecutionEvent::ThinkingToken {
                node_id,
                node_type,
                token,
            } => Some(json!({
                "type": "thinking-delta",
                "node_id": node_id,
                "node_type": node_type,
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
            DagExecutionEvent::LlmToolCall {
                tool_id,
                args_chunk,
                ..
            } => Some(json!({
                "type": "tool-input-delta",
                "toolCallId": tool_id,
                "inputTextDelta": args_chunk
            })),
            DagExecutionEvent::LlmToolCallStart {
                tool_id,
                tool_name,
                tool_args,
                ..
            } => {
                let input = serde_json::from_str::<Value>(tool_args)
                    .unwrap_or_else(|_| Value::String(tool_args.clone()));
                Some(json!({
                    "type": "tool-input-available",
                    "toolCallId": tool_id,
                    "toolName": tool_name,
                    "input": input
                }))
            }
            DagExecutionEvent::LlmToolCallFinish {
                tool_id, output, ..
            } => {
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
                self.seen_top_tool_ids.clear();
                self.seen_sub_tool_ids.clear();

                let finish_reason = output
                    .get("__colmena_status")
                    .or_else(|| {
                        output
                            .get("extra_info")
                            .and_then(|e| e.get("__colmena_status"))
                    })
                    .and_then(|s| s.as_str())
                    .map(|s| {
                        if s == "SUSPENDED" {
                            "suspended"
                        } else {
                            "stop"
                        }
                    })
                    .unwrap_or("stop");

                let usage_obj = self.usage_snapshot();

                Some(json!({
                    "type": "finish",
                    "finishReason": finish_reason,
                    "usage": usage_obj,
                    "output": output
                }))
            }
            // Turn boundaries are forwarded as a lightweight `agent-turn` frame
            // (never `finish`/`error`, which terminate the stream). This keeps the
            // downstream no-event watchdog fed across quiet turn transitions and
            // gives the client a turn signal. `level`/`path` are injected below.
            DagExecutionEvent::LlmMessageStart { node_id } => Some(json!({
                "type": "agent-turn",
                "phase": "start",
                "node_id": node_id
            })),
            DagExecutionEvent::LlmMessageFinish { node_id, .. } => Some(json!({
                "type": "agent-turn",
                "phase": "finish",
                "node_id": node_id
            })),
            DagExecutionEvent::Error { message } => Some(json!({
                "type": "error",
                "errorText": message
            })),
            DagExecutionEvent::Cancelled {
                reason,
                partial_output,
            } => {
                // Close any still-open text blocks before terminating.
                for part_id in self.text_block_ids.values() {
                    parts.push(json!({ "type": "text-end", "id": part_id }));
                }
                self.text_block_ids.clear();
                self.seen_top_tool_ids.clear();
                self.seen_sub_tool_ids.clear();

                // UX frame: explicit "stopped by user" signal for the frontend.
                parts.push(json!({
                    "type": "cancelled",
                    "reason": reason,
                    "output": partial_output
                }));

                // Terminator the frontend already respects (closes the stream).
                // Uses the same snapshot as `GraphFinish`: a cancelled run used
                // real tokens and must report its cache split too.
                let usage_obj = self.usage_snapshot();
                Some(json!({
                    "type": "finish",
                    "finishReason": "cancelled",
                    "usage": usage_obj,
                    "output": partial_output
                }))
            }
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
            DagExecutionEvent::ToolDescribed {
                node_id,
                tool_id,
                tool_name,
            } => Some(json!({
                "type": "tool-described",
                "nodeId": node_id,
                "toolCallId": tool_id,
                "toolName": tool_name,
            })),
            DagExecutionEvent::Progress { node_id, idle_secs } => Some(json!({
                "type": "status",
                "stage": "running",
                "node_id": node_id,
                "idleSecs": idle_secs
            })),
            DagExecutionEvent::BatchProgress {
                node_id,
                total,
                completed,
                ok,
                err,
                in_flight,
            } => Some(json!({
                "type": "batch-progress",
                "nodeId": node_id,
                "total": total,
                "completed": completed,
                "ok": ok,
                "err": err,
                "inFlight": in_flight,
            })),
            DagExecutionEvent::BatchItemFinished {
                node_id,
                index,
                key,
                status,
            } => Some(json!({
                "type": "batch-item-finished",
                "nodeId": node_id,
                "index": index,
                "key": key,
                "status": status,
            })),
            DagExecutionEvent::SubgraphWrapped { inner, .. } => match Self::deep_base(inner) {
                DagExecutionEvent::NodeStart {
                    node_id,
                    node_type,
                    inputs,
                    config,
                } => {
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
                DagExecutionEvent::LlmToken { token, .. } => {
                    let part_id = self
                        .text_block_ids
                        .get(&path)
                        .cloned()
                        .unwrap_or_else(|| path.clone());
                    Some(json!({ "type": "subgraph-text-delta", "id": part_id, "delta": token }))
                }
                // Same `thinking-delta` type as the unwrapped case, on purpose.
                // An orchestrator's internal planner/critic reasoning is thinking
                // at every nesting level; `level` and `path` place it in the tree,
                // so it needs no `subgraph-` variant — and the events reference
                // states outright that thinking frames are not subgraph events.
                //
                // It used to map to `subgraph-text-delta`, which silently
                // reclassified that reasoning as the agent's user-facing answer.
                DagExecutionEvent::ThinkingToken {
                    node_id,
                    node_type,
                    token,
                } => Some(json!({
                    "type": "thinking-delta",
                    "node_id": node_id,
                    "node_type": node_type,
                    "delta": token
                })),
                DagExecutionEvent::LlmToolCall {
                    tool_id,
                    args_chunk,
                    ..
                } => Some(json!({
                    "type": "subgraph-tool-input-delta",
                    "toolCallId": tool_id,
                    "inputTextDelta": args_chunk
                })),
                DagExecutionEvent::LlmToolCallStart {
                    tool_id,
                    tool_name,
                    tool_args,
                    ..
                } => {
                    let input = serde_json::from_str::<Value>(tool_args)
                        .unwrap_or_else(|_| Value::String(tool_args.clone()));
                    Some(json!({
                        "type": "subgraph-tool-input-available",
                        "toolCallId": tool_id,
                        "toolName": tool_name,
                        "input": input
                    }))
                }
                DagExecutionEvent::LlmToolCallFinish {
                    tool_id, output, ..
                } => {
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
                DagExecutionEvent::Progress { node_id, idle_secs } => Some(json!({
                    "type": "status",
                    "stage": "running",
                    "node_id": node_id,
                    "idleSecs": idle_secs
                })),
                DagExecutionEvent::ToolDescribed {
                    node_id,
                    tool_id,
                    tool_name,
                } => Some(json!({
                    "type": "subgraph-tool-described",
                    "nodeId": node_id,
                    "toolCallId": tool_id,
                    "toolName": tool_name,
                })),
                DagExecutionEvent::BatchProgress {
                    node_id,
                    total,
                    completed,
                    ok,
                    err,
                    in_flight,
                } => Some(json!({
                    "type": "subgraph-batch-progress",
                    "nodeId": node_id,
                    "total": total,
                    "completed": completed,
                    "ok": ok,
                    "err": err,
                    "inFlight": in_flight,
                })),
                DagExecutionEvent::BatchItemFinished {
                    node_id,
                    index,
                    key,
                    status,
                } => Some(json!({
                    "type": "subgraph-batch-item-finished",
                    "nodeId": node_id,
                    "index": index,
                    "key": key,
                    "status": status,
                })),
                // Turn boundaries from a sub-agent — forwarded for visibility and
                // to keep the no-event watchdog fed. `level`/`path` injected below.
                DagExecutionEvent::LlmMessageStart { node_id } => Some(json!({
                    "type": "agent-turn",
                    "phase": "start",
                    "node_id": node_id
                })),
                DagExecutionEvent::LlmMessageFinish { node_id, .. } => Some(json!({
                    "type": "agent-turn",
                    "phase": "finish",
                    "node_id": node_id
                })),
                _ => None,
            },
        };

        if let Some(v) = protocol {
            parts.push(v);
        }

        // Tag every frame for this event with its nesting `level` and lineage
        // `path` (additive; existing consumers ignore unknown fields). All parts
        // produced in this call pertain to the same source event, hence share the
        // same (level, path). `or_insert` never clobbers a frame that set them.
        for part in parts.iter_mut() {
            if let Some(obj) = part.as_object_mut() {
                obj.entry("level").or_insert_with(|| json!(level));
                obj.entry("path").or_insert_with(|| json!(path.clone()));
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag_engine::domain::events::DagExecutionEvent;

    fn tool_call_sequence() -> Vec<DagExecutionEvent> {
        vec![
            DagExecutionEvent::LlmToolCall {
                node_id: "llm_1".into(),
                tool_id: "call_abc".into(),
                tool_name: "getWeather".into(),
                args_chunk: "{\"city\"".into(),
            },
            DagExecutionEvent::LlmToolCall {
                node_id: "llm_1".into(),
                tool_id: "call_abc".into(),
                tool_name: "getWeather".into(),
                args_chunk: ":\"SF\"}".into(),
            },
            DagExecutionEvent::LlmToolCallStart {
                node_id: "llm_1".into(),
                tool_id: "call_abc".into(),
                tool_name: "getWeather".into(),
                tool_args: "{\"city\":\"SF\"}".into(),
            },
            DagExecutionEvent::LlmToolCallFinish {
                node_id: "llm_1".into(),
                tool_id: "call_abc".into(),
                success: true,
                output: "{\"weather\":\"sunny\"}".into(),
            },
        ]
    }

    #[test]
    fn test_tool_input_start_emitted_once_before_first_delta() {
        let mut mapper = SseMapper::new();
        let events = tool_call_sequence();

        let parts = mapper.map(&events[0]);
        assert_eq!(
            parts.len(),
            2,
            "expected [tool-input-start, tool-input-delta]"
        );
        assert_eq!(parts[0]["type"], "tool-input-start");
        assert_eq!(parts[0]["toolCallId"], "call_abc");
        assert_eq!(parts[0]["toolName"], "getWeather");
        assert_eq!(parts[1]["type"], "tool-input-delta");

        let parts2 = mapper.map(&events[1]);
        assert_eq!(parts2.len(), 1, "expected only tool-input-delta on repeat");
        assert_eq!(parts2[0]["type"], "tool-input-delta");
    }

    #[test]
    fn test_tool_input_available_and_output() {
        let mut mapper = SseMapper::new();
        let events = tool_call_sequence();

        mapper.map(&events[0]); // warm up seen_tool_ids

        let parts = mapper.map(&events[2]);
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0]["type"], "tool-input-available");
        assert_eq!(parts[0]["toolName"], "getWeather");

        let parts = mapper.map(&events[3]);
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0]["type"], "tool-output-available");
    }

    #[test]
    fn test_subgraph_tool_input_start_emitted_once() {
        let mut mapper = SseMapper::new();
        let event1 = DagExecutionEvent::SubgraphWrapped {
            inner: Box::new(DagExecutionEvent::LlmToolCall {
                node_id: "inner_llm".into(),
                tool_id: "call_xyz".into(),
                tool_name: "search".into(),
                args_chunk: "{\"q\"".into(),
            }),
            depth: 1,
            path: "top>inner_llm".into(),
        };
        let event2 = DagExecutionEvent::SubgraphWrapped {
            inner: Box::new(DagExecutionEvent::LlmToolCall {
                node_id: "inner_llm".into(),
                tool_id: "call_xyz".into(),
                tool_name: "search".into(),
                args_chunk: ":\"rust\"}".into(),
            }),
            depth: 1,
            path: "top>inner_llm".into(),
        };

        // First chunk → subgraph-tool-input-start + subgraph-tool-input-delta
        let parts = mapper.map(&event1);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0]["type"], "subgraph-tool-input-start");
        assert_eq!(parts[0]["toolCallId"], "call_xyz");
        assert_eq!(parts[1]["type"], "subgraph-tool-input-delta");

        // Second chunk (same tool_id, different args) → only subgraph-tool-input-delta
        let parts2 = mapper.map(&event2);
        assert_eq!(parts2.len(), 1);
        assert_eq!(parts2[0]["type"], "subgraph-tool-input-delta");
    }

    #[test]
    fn test_top_level_and_subgraph_tool_ids_are_independent() {
        let mut mapper = SseMapper::new();
        let shared_tool_id = "call_shared";

        // Top-level tool call fires first
        let top_event = DagExecutionEvent::LlmToolCall {
            node_id: "llm".into(),
            tool_id: shared_tool_id.into(),
            tool_name: "search".into(),
            args_chunk: "{\"q\":\"x\"}".into(),
        };
        let top_parts = mapper.map(&top_event);
        assert_eq!(top_parts.len(), 2);
        assert_eq!(top_parts[0]["type"], "tool-input-start");

        // Subgraph tool call with SAME tool_id — must still emit subgraph-tool-input-start
        let sub_event = DagExecutionEvent::SubgraphWrapped {
            inner: Box::new(DagExecutionEvent::LlmToolCall {
                node_id: "inner_llm".into(),
                tool_id: shared_tool_id.into(),
                tool_name: "search".into(),
                args_chunk: "{\"q\":\"y\"}".into(),
            }),
            depth: 1,
            path: "top>inner_llm".into(),
        };
        let sub_parts = mapper.map(&sub_event);
        assert_eq!(
            sub_parts.len(),
            2,
            "subgraph tool-input-start must not be suppressed by top-level seen_tool_ids"
        );
        assert_eq!(sub_parts[0]["type"], "subgraph-tool-input-start");
    }

    /// Feeds one usage event and returns the `finish` frame's `usage` object.
    fn usage_after(ev: DagExecutionEvent, terminator: DagExecutionEvent) -> Value {
        let mut mapper = SseMapper::new();
        mapper.map(&ev);
        mapper
            .map(&terminator)
            .into_iter()
            .find(|p| p["type"] == "finish")
            .expect("must emit a finish terminator")["usage"]
            .clone()
    }

    fn usage_event(
        prompt: u32,
        completion: u32,
        read: Option<u32>,
        write: Option<u32>,
    ) -> DagExecutionEvent {
        DagExecutionEvent::LlmUsage {
            node_id: "llm_1".into(),
            prompt_tokens: prompt,
            completion_tokens: completion,
            thinking_tokens: None,
            cache_read_tokens: read,
            cache_write_tokens: write,
        }
    }

    #[test]
    fn finish_usage_keeps_existing_field_names() {
        // The wire contract: these four keys must never be renamed, because
        // downstream billing binds to them.
        let usage = usage_after(
            usage_event(404, 8, Some(1809), None),
            DagExecutionEvent::GraphFinish { output: json!({}) },
        );
        for key in [
            "promptTokens",
            "completionTokens",
            "totalTokens",
            "cacheReadTokens",
        ] {
            assert!(usage.get(key).is_some(), "missing expected key `{key}`");
        }
    }

    #[test]
    fn finish_usage_counts_cache_in_the_total() {
        // Real Anthropic numbers, measured live 2026-08-23. The old total was
        // 412 and hid 1809 billed cache-read tokens.
        let usage = usage_after(
            usage_event(404, 8, Some(1809), None),
            DagExecutionEvent::GraphFinish { output: json!({}) },
        );
        assert_eq!(usage["promptTokens"], 404, "prompt stays fresh-only");
        assert_eq!(usage["cacheReadTokens"], 1809);
        assert_eq!(usage["totalTokens"], 404 + 8 + 1809);
    }

    #[test]
    fn finish_usage_always_carries_both_cache_fields_even_at_zero() {
        // An absent field could not be distinguished from a provider that never
        // reports one, so both are emitted unconditionally.
        let usage = usage_after(
            usage_event(100, 10, None, None),
            DagExecutionEvent::GraphFinish { output: json!({}) },
        );
        assert_eq!(usage["cacheReadTokens"], 0);
        assert_eq!(usage["cacheWriteTokens"], 0);
        assert_eq!(usage["totalTokens"], 110);
    }

    #[test]
    fn cancelled_finish_reports_the_same_usage_shape_as_graph_finish() {
        // A cancelled run burned real tokens; its terminator used to drop the
        // cache and thinking columns entirely.
        let ev = usage_event(404, 8, Some(1809), Some(200));
        let finished = usage_after(
            ev.clone(),
            DagExecutionEvent::GraphFinish { output: json!({}) },
        );
        let cancelled = usage_after(
            ev,
            DagExecutionEvent::Cancelled {
                reason: Some("stopped".into()),
                partial_output: json!({}),
            },
        );
        assert_eq!(finished, cancelled);
        assert_eq!(cancelled["cacheWriteTokens"], 200);
    }

    #[test]
    fn cancelled_maps_to_cancelled_then_finish() {
        let mut mapper = SseMapper::new();
        let ev = DagExecutionEvent::Cancelled {
            reason: Some("stopped".into()),
            partial_output: json!({ "n1": { "output": 1 } }),
        };
        let parts = mapper.map(&ev);

        // Expect a UX `cancelled` frame followed by a `finish` terminator.
        let cancelled = parts
            .iter()
            .find(|p| p["type"] == "cancelled")
            .expect("must emit a cancelled frame");
        assert_eq!(cancelled["reason"], "stopped");
        assert_eq!(cancelled["output"]["n1"]["output"], 1);

        let finish = parts
            .iter()
            .find(|p| p["type"] == "finish")
            .expect("must emit a finish terminator");
        assert_eq!(finish["finishReason"], "cancelled");
    }

    #[test]
    fn progress_maps_to_status_running_part() {
        let mut mapper = SseMapper::new();
        let parts = mapper.map(&DagExecutionEvent::Progress {
            node_id: "n1".to_string(),
            idle_secs: 25,
        });
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0]["type"], "status");
        assert_eq!(parts[0]["stage"], "running");
        assert_eq!(parts[0]["node_id"], "n1");
        assert_eq!(parts[0]["idleSecs"], 25);
    }

    /// Regression (Fase A): a *doubly*-nested `SubgraphWrapped` (a grandchild
    /// event, level 2) must still produce a `subgraph-text-delta` frame with
    /// `level: 2`. Before the fix the mapper only unwrapped one level and the
    /// nested wrapper fell into `_ => None`, dropping the event entirely.
    #[test]
    fn double_nested_wrapped_llm_token_maps_to_subgraph_text_delta_level_2() {
        let mut mapper = SseMapper::new();
        let ev = DagExecutionEvent::SubgraphWrapped {
            inner: Box::new(DagExecutionEvent::SubgraphWrapped {
                inner: Box::new(DagExecutionEvent::LlmToken {
                    node_id: "calc_a".into(),
                    token: "96".into(),
                }),
                depth: 1,
                path: "orch>calc_a".into(),
            }),
            depth: 1,
            path: "Builder>orch>calc_a".into(),
        };
        let parts = mapper.map(&ev);

        // [subgraph-text-start, subgraph-text-delta] — both at level 2.
        let delta = parts
            .iter()
            .find(|p| p["type"] == "subgraph-text-delta")
            .expect("double-nested LlmToken must produce a subgraph-text-delta (was dropped)");
        assert_eq!(delta["delta"], "96");
        assert_eq!(delta["level"], 2, "accumulated depth across two layers");
        assert_eq!(delta["path"], "Builder>orch>calc_a");
        for p in &parts {
            assert_eq!(p["level"], 2, "every frame for this event is level 2");
            assert_eq!(p["path"], "Builder>orch>calc_a");
        }
    }

    /// Fase B: level-0 (non-wrapped) frames carry `level: 0` and `path` = node id.
    #[test]
    fn level_zero_frames_carry_level_and_path() {
        let mut mapper = SseMapper::new();
        let parts = mapper.map(&DagExecutionEvent::LlmToken {
            node_id: "top_llm".into(),
            token: "hi".into(),
        });
        let delta = parts
            .iter()
            .find(|p| p["type"] == "text-delta")
            .expect("must emit text-delta");
        assert_eq!(delta["level"], 0);
        assert_eq!(delta["path"], "top_llm");
    }

    /// Fase C5: turn-boundary events forward as a lightweight `agent-turn` frame
    /// (Some), never `finish`/`error`. Both the top-level and wrapped variants.
    #[test]
    fn message_boundaries_forward_as_agent_turn() {
        let mut mapper = SseMapper::new();
        let start = mapper.map(&DagExecutionEvent::LlmMessageStart {
            node_id: "n1".into(),
        });
        assert_eq!(start.len(), 1, "boundary must forward (was dropped)");
        assert_eq!(start[0]["type"], "agent-turn");
        assert_eq!(start[0]["phase"], "start");
        assert_eq!(start[0]["level"], 0);

        let finish = mapper.map(&DagExecutionEvent::LlmMessageFinish {
            node_id: "n1".into(),
            usage: None,
        });
        assert_eq!(finish[0]["type"], "agent-turn");
        assert_eq!(finish[0]["phase"], "finish");

        // Wrapped boundary → still agent-turn, carries the sub-agent level.
        let wrapped = mapper.map(&DagExecutionEvent::SubgraphWrapped {
            inner: Box::new(DagExecutionEvent::LlmMessageStart {
                node_id: "sub".into(),
            }),
            depth: 2,
            path: "Builder>orch>sub".into(),
        });
        assert_eq!(wrapped[0]["type"], "agent-turn");
        assert_eq!(wrapped[0]["level"], 2);
        assert_eq!(wrapped[0]["path"], "Builder>orch>sub");

        // Never a stream terminator.
        assert_ne!(start[0]["type"], "finish");
        assert_ne!(start[0]["type"], "error");
    }

    #[test]
    fn wrapped_progress_maps_to_status_running_part() {
        let mut mapper = SseMapper::new();
        let parts = mapper.map(&DagExecutionEvent::SubgraphWrapped {
            inner: Box::new(DagExecutionEvent::Progress {
                node_id: "inner_node".to_string(),
                idle_secs: 40,
            }),
            depth: 1,
            path: "top>inner_node".into(),
        });
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0]["type"], "status");
        assert_eq!(parts[0]["stage"], "running");
        assert_eq!(parts[0]["node_id"], "inner_node");
        assert_eq!(parts[0]["idleSecs"], 40);
    }

    // ── Nested thinking frames (defect: planner split across two levels) ─────

    fn wrap(inner: DagExecutionEvent, depth: u32, path: &str) -> DagExecutionEvent {
        DagExecutionEvent::SubgraphWrapped {
            inner: Box::new(inner),
            depth,
            path: path.into(),
        }
    }

    fn thinking(node_id: &str, token: &str) -> DagExecutionEvent {
        DagExecutionEvent::ThinkingToken {
            node_id: node_id.into(),
            node_type: "planner".into(),
            token: token.into(),
        }
    }

    /// A wrapped ThinkingToken is internal LLM reasoning, not the agent's
    /// answer. It must NOT map to `subgraph-text-delta` and must NOT open a text
    /// block; it keeps the same `thinking-delta` type as the unwrapped case, so
    /// no consumer has to learn a new one.
    #[test]
    fn wrapped_thinking_token_stays_a_thinking_delta() {
        let mut mapper = SseMapper::new();
        let parts = mapper.map(&wrap(thinking("planner", "hmm"), 1, "orch>planner"));

        assert_eq!(
            parts.len(),
            1,
            "must not also emit a text-start; got {parts:?}"
        );
        assert_eq!(parts[0]["type"], "thinking-delta");
        assert_eq!(parts[0]["node_id"], "planner");
        assert_eq!(parts[0]["node_type"], "planner");
        assert_eq!(parts[0]["delta"], "hmm");
        assert!(
            parts[0].get("id").is_none(),
            "thinking frames carry no text-block id"
        );
    }

    /// The orchestrator-inside-a-subgraph shape: the planner's node-start is
    /// wrapped twice (level 2), so its thinking must arrive at level 2 under the
    /// same path. Before the fix the token arrived at level 1 under `top>planner`
    /// while its node-start sat at level 2 under `top>orch>planner`.
    #[test]
    fn nested_planner_thinking_shares_level_and_path_with_its_node_start() {
        let mut mapper = SseMapper::new();

        let start = wrap(
            wrap(
                DagExecutionEvent::NodeStart {
                    node_id: "planner".into(),
                    node_type: "planner".into(),
                    inputs: json!({}),
                    config: json!({}),
                },
                1,
                "orch>planner",
            ),
            1,
            "top>orch>planner",
        );
        let token = wrap(
            wrap(thinking("planner", "step 1"), 1, "orch>planner"),
            1,
            "top>orch>planner",
        );

        let start_parts = mapper.map(&start);
        let token_parts = mapper.map(&token);

        assert_eq!(start_parts[0]["type"], "subgraph-node-start");
        assert_eq!(token_parts[0]["type"], "thinking-delta");
        assert_eq!(
            start_parts[0]["level"], token_parts[0]["level"],
            "a node's thinking must sit at the same nesting level as its node-start"
        );
        assert_eq!(
            start_parts[0]["path"], token_parts[0]["path"],
            "a node's thinking must share its node-start's lineage path"
        );
        assert_eq!(token_parts[0]["level"], 2);
        assert_eq!(token_parts[0]["path"], "top>orch>planner");
    }

    // ── Text blocks keyed by path, not node_id ──────────────────────────────

    fn llm_token(node_id: &str, token: &str) -> DagExecutionEvent {
        DagExecutionEvent::LlmToken {
            node_id: node_id.into(),
            token: token.into(),
        }
    }

    fn node_finish(node_id: &str) -> DagExecutionEvent {
        DagExecutionEvent::NodeFinish {
            node_id: node_id.into(),
            output: json!({}),
        }
    }

    /// Two sub-agents in different branches sharing a node id (the same
    /// `child_graph_inline` instantiated twice) must get independent text
    /// blocks. Keyed by node id, agent B's open clobbered agent A's uuid, A's
    /// finish closed B's block, and B's block leaked forever.
    #[test]
    fn same_node_id_in_different_branches_gets_independent_text_blocks() {
        let mut mapper = SseMapper::new();

        let a_open = mapper.map(&wrap(llm_token("llm_1", "a"), 1, "top>agent_a>llm_1"));
        let b_open = mapper.map(&wrap(llm_token("llm_1", "b"), 1, "top>agent_b>llm_1"));

        assert_eq!(a_open[0]["type"], "subgraph-text-start");
        assert_eq!(b_open[0]["type"], "subgraph-text-start");
        let a_id = a_open[0]["id"].clone();
        let b_id = b_open[0]["id"].clone();
        assert_ne!(a_id, b_id, "each branch owns a distinct text block");

        // Closing A must close A's block, leaving B's open.
        let a_close = mapper.map(&wrap(node_finish("llm_1"), 1, "top>agent_a>llm_1"));
        assert_eq!(a_close[0]["type"], "subgraph-text-end");
        assert_eq!(
            a_close[0]["id"], a_id,
            "A's finish must close A's own block"
        );

        let b_close = mapper.map(&wrap(node_finish("llm_1"), 1, "top>agent_b>llm_1"));
        assert_eq!(
            b_close[0]["id"], b_id,
            "B's block must still be open and close with its own id"
        );
    }

    /// Level-0 behavior is unchanged: with no wrapper the path falls back to the
    /// node id, so open/close pair exactly as before.
    #[test]
    fn top_level_text_block_still_opens_and_closes_by_node() {
        let mut mapper = SseMapper::new();
        let open = mapper.map(&llm_token("top_llm", "hi"));
        assert_eq!(open[0]["type"], "text-start");
        let close = mapper.map(&node_finish("top_llm"));
        assert_eq!(close[0]["type"], "text-end");
        assert_eq!(close[0]["id"], open[0]["id"]);
    }
    /// A node dropped by the engine must reach the wire. Three engine paths
    /// discard a node without executing it (unsatisfiable dependency, `null`
    /// output, unresolved edge pointer); two of them are intentional control
    /// flow, so the frame is informational — but it must exist, otherwise a
    /// mis-wired checkpoint looks identical to a graph that ran clean.
    #[test]
    fn node_skipped_maps_to_a_wire_frame_with_its_reason() {
        let mut m = SseMapper::new();
        let parts = m.map(&DagExecutionEvent::NodeSkipped {
            node_id: "ask_user".into(),
            reason: "pointer_unresolved".into(),
        });
        assert_eq!(parts.len(), 1, "expected exactly one frame, got: {parts:?}");
        assert_eq!(parts[0]["type"], "node-skipped");
        assert_eq!(parts[0]["node_id"], "ask_user");
        assert_eq!(parts[0]["reason"], "pointer_unresolved");
    }

    #[test]
    fn node_skipped_carries_level_and_path_like_every_other_node_frame() {
        let mut m = SseMapper::new();
        let parts = m.map(&DagExecutionEvent::NodeSkipped {
            node_id: "ask_user".into(),
            reason: "upstream_never_fired".into(),
        });
        assert_eq!(parts[0]["level"], 0);
        assert_eq!(
            parts[0]["path"], "ask_user",
            "path falls back to the node id, so the frame is placeable in the nested tree"
        );
    }
}
