//! `list_threads` synthetic tool — enumerate the conversation threads of any
//! `memory_mode: "dynamic"` tool so the model can navigate and continue one.
//! Mirrors the `recall_history` wiring: a `with_conversation_history(repo, key)`
//! builder supplies the deps; the dispatch arm intercepts the tool name.

use crate::llm::domain::tools::ToolDefinition;
use crate::llm::domain::{ConversationKey, ConversationRepository, NodeActivity};
use schemars::JsonSchema;
use serde::Deserialize;
use std::sync::Arc;

pub const TOOL_LIST_THREADS: &str = "list_threads";
const OPENING_MAX_CHARS: usize = 120;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListThreadsArgs {
    /// Optional: name of a specific dynamic tool to list. Omit to list every
    /// dynamic tool's threads, grouped by tool.
    #[serde(default)]
    pub tool: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ThreadInfo {
    pub thread_id: String,
    pub messages: i64,
    pub last_activity: String,
    pub opening: Option<String>,
}

pub fn tool_list_threads() -> ToolDefinition {
    use crate::text;
    super::build_synthetic_tool_with_summary::<ListThreadsArgs>(
        TOOL_LIST_THREADS,
        text::tool_description(TOOL_LIST_THREADS),
        text::tool_summary(TOOL_LIST_THREADS),
    )
}

/// Group per-node_id rows into per-thread entries. `node_id` is
/// `tool/<tool_name>/<thread_id>[/<child...>]`; the thread id is the first
/// segment after the `tool/<tool_name>/` prefix. Rows sharing a thread id merge
/// (sum messages, max last_activity, opening from the earliest source row).
fn aggregate_threads(tool_name: &str, rows: Vec<NodeActivity>) -> Vec<ThreadInfo> {
    use std::collections::HashMap;
    let prefix = format!("tool/{tool_name}/");
    // thread_id -> (messages, max_last, best_opening, best_opening_key)
    let mut acc: HashMap<String, ThreadInfo> = HashMap::new();
    for r in rows {
        let Some(rest) = r.node_id.strip_prefix(&prefix) else {
            continue;
        };
        let thread_id = rest.split('/').next().unwrap_or(rest).to_string();
        if thread_id.is_empty() {
            continue;
        }
        let opening = r.opening.map(|o| truncate(&o, OPENING_MAX_CHARS));
        let e = acc.entry(thread_id.clone()).or_insert(ThreadInfo {
            thread_id,
            messages: 0,
            last_activity: String::new(),
            opening: None,
        });
        e.messages += r.message_count;
        if r.last_activity > e.last_activity {
            e.last_activity = r.last_activity.clone();
        }
        // keep the opening from the lexicographically-earliest node_id as a stable
        // "first source" proxy; fill if still empty
        if e.opening.is_none() {
            e.opening = opening;
        }
    }
    let mut out: Vec<ThreadInfo> = acc.into_values().collect();
    out.sort_by(|a, b| b.last_activity.cmp(&a.last_activity));
    out
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut t: String = s.chars().take(max).collect();
    t.push('…');
    t
}

/// Dispatch a `list_threads` call. `dynamic_tool_names` is the set of configured
/// tools whose `memory_mode == Dynamic`. Returns a serde_json value for the LLM.
pub async fn dispatch_list_threads(
    repo: &Arc<dyn ConversationRepository>,
    key: &ConversationKey,
    dynamic_tool_names: &[String],
    args: serde_json::Value,
) -> serde_json::Value {
    let parsed: ListThreadsArgs = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => return serde_json::json!({ "error": format!("invalid_args: {e}") }),
    };
    let targets: Vec<String> = match parsed.tool {
        Some(t) if dynamic_tool_names.iter().any(|n| n == &t) => vec![t],
        Some(t) => {
            return serde_json::json!({
                "error": format!("unknown_or_non_dynamic_tool: '{t}'"),
                "available_dynamic_tools": dynamic_tool_names,
            });
        }
        None => dynamic_tool_names.to_vec(),
    };
    let keying = key.keying();
    let mut tools_json = Vec::new();
    for name in targets {
        let prefix = format!("tool/{name}/");
        let rows = match repo.list_node_activity(keying, &prefix).await {
            Ok(r) => r,
            Err(e) => return serde_json::json!({ "error": format!("query_failed: {e}") }),
        };
        let threads = aggregate_threads(&name, rows);
        tools_json.push(serde_json::json!({ "tool": name, "threads": threads }));
    }
    serde_json::json!({ "tools": tools_json })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::domain::NodeActivity;

    fn na(node_id: &str, n: i64, last: &str, opening: &str) -> NodeActivity {
        NodeActivity {
            node_id: node_id.into(),
            message_count: n,
            last_activity: last.into(),
            opening: Some(opening.into()),
        }
    }

    #[test]
    fn aggregate_extracts_thread_id_and_merges_children() {
        let rows = vec![
            na(
                "tool/archivador/alfa/keeper",
                4,
                "2026-08-24T10:00:00Z",
                "abrir alfa",
            ),
            na(
                "tool/archivador/alfa/notes",
                2,
                "2026-08-24T11:00:00Z",
                "z-later",
            ), // same thread, 2nd child
            na(
                "tool/archivador/beta/keeper",
                3,
                "2026-08-24T09:00:00Z",
                "abrir beta",
            ),
        ];
        let out = aggregate_threads("archivador", rows);
        // sorted by last_activity desc → alfa (11:00) before beta (09:00)
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].thread_id, "alfa");
        assert_eq!(out[0].messages, 6); // merged 4 + 2
        assert_eq!(out[0].opening.as_deref(), Some("abrir alfa")); // earliest source
        assert_eq!(out[1].thread_id, "beta");
    }

    #[test]
    fn aggregate_handles_bare_llm_call_thread_without_child_suffix() {
        let rows = vec![na("tool/asesor/caso-12", 5, "2026-08-24T12:00:00Z", "hola")];
        let out = aggregate_threads("asesor", rows);
        assert_eq!(out[0].thread_id, "caso-12");
    }
}
