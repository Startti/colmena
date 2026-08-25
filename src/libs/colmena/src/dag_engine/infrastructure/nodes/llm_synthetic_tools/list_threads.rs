//! `list_threads` synthetic tool — enumerate the conversation threads of any
//! `memory_mode: "dynamic"` tool so the model can navigate and continue one.
//! Mirrors the `recall_history` wiring: a `with_conversation_history(repo, key)`
//! builder supplies the deps; the dispatch arm intercepts the tool name.

use crate::llm::domain::tools::ToolDefinition;
use crate::llm::domain::{
    ConversationKey, ConversationRepository, NodeActivity, MAX_LISTED_NODE_ACTIVITY,
};
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
/// (sum messages, max last_activity, opening from the row with the
/// lexicographically-smallest `node_id`).
///
/// Neither the Postgres/SQLite backends (`GROUP BY node_id` with no outer
/// `ORDER BY`) nor the in-memory backend (`HashMap` iteration) guarantee an
/// input order, so `rows` is sorted by `node_id` up front — this is NOT "the
/// earliest by time" (`NodeActivity` carries no first-activity timestamp to
/// order by), only a stable, deterministic tie-break so repeated calls return
/// the same `opening` for a given thread.
fn aggregate_threads(tool_name: &str, mut rows: Vec<NodeActivity>) -> Vec<ThreadInfo> {
    use std::collections::HashMap;
    rows.sort_by(|a, b| a.node_id.cmp(&b.node_id));
    let prefix = format!("tool/{tool_name}/");
    // thread_id -> (messages, max_last, best_opening, best_opening_key)
    let mut acc: HashMap<String, ThreadInfo> = HashMap::new();
    for r in rows {
        let Some(rest) = r.node_id.strip_prefix(&prefix) else {
            continue;
        };
        // `str::split` always yields at least one item (the whole string when
        // there's no separator), so the first segment is always present —
        // no `unwrap_or` fallback is reachable here.
        let thread_id = rest.split('/').next().unwrap().to_string();
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
        // `rows` is sorted by node_id above, so the first row we see for a
        // given thread_id is the one with the smallest node_id; keep its
        // opening (fill-if-still-empty makes this a first-write-wins).
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
        // The backend caps rows at MAX_LISTED_NODE_ACTIVITY; hitting the cap
        // means there may be more threads/children than shown, so flag it
        // for the model rather than silently returning a partial list.
        let truncated = rows.len() >= MAX_LISTED_NODE_ACTIVITY as usize;
        let threads = aggregate_threads(&name, rows);
        let mut entry = serde_json::json!({ "tool": name, "threads": threads });
        if truncated {
            entry["truncated"] = serde_json::Value::Bool(true);
        }
        tools_json.push(entry);
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
                                        // "alfa/keeper" < "alfa/notes" lexicographically, so keeper's opening wins
        assert_eq!(out[0].opening.as_deref(), Some("abrir alfa"));
        assert_eq!(out[1].thread_id, "beta");
    }

    #[test]
    fn aggregate_handles_bare_llm_call_thread_without_child_suffix() {
        let rows = vec![na("tool/asesor/caso-12", 5, "2026-08-24T12:00:00Z", "hola")];
        let out = aggregate_threads("asesor", rows);
        assert_eq!(out[0].thread_id, "caso-12");
    }

    #[test]
    fn aggregate_opening_is_deterministic_regardless_of_input_order() {
        // Same rows as `aggregate_extracts_thread_id_and_merges_children` but
        // fed in reverse order — backends (Postgres/SQLite GROUP BY with no
        // ORDER BY, in-memory HashMap iteration) make no input-order promise,
        // so the merged `opening` must not depend on it.
        let rows = vec![
            na(
                "tool/archivador/alfa/notes",
                2,
                "2026-08-24T11:00:00Z",
                "z-later",
            ),
            na(
                "tool/archivador/alfa/keeper",
                4,
                "2026-08-24T10:00:00Z",
                "abrir alfa",
            ),
        ];
        let out = aggregate_threads("archivador", rows);
        assert_eq!(out[0].thread_id, "alfa");
        assert_eq!(out[0].opening.as_deref(), Some("abrir alfa"));
    }

    #[test]
    fn truncate_long_opening_gets_ellipsis_without_panicking_on_char_boundary() {
        // 130 accented/multi-byte chars — a byte-index truncation (e.g.
        // `s[..max]`) would panic here on a non-boundary; `truncate` walks
        // `chars()` so it must not.
        let long = "café ".repeat(26); // 5 chars * 26 = 130 chars, all multi-byte-safe via chars()
        assert_eq!(long.chars().count(), 130);
        let t = truncate(&long, OPENING_MAX_CHARS);
        assert_eq!(t.chars().count(), OPENING_MAX_CHARS + 1); // + ellipsis char
        assert!(t.ends_with('…'));
        assert_eq!(
            t.chars().take(OPENING_MAX_CHARS).collect::<String>(),
            long.chars().take(OPENING_MAX_CHARS).collect::<String>()
        );
    }

    #[test]
    fn truncate_short_opening_is_unchanged() {
        let s = "hola";
        assert_eq!(truncate(s, OPENING_MAX_CHARS), s);
    }

    // --- dispatch_list_threads tests (Finding MINOR 8) ---

    use crate::llm::domain::{Conversation, LlmError, LlmMessage};
    use crate::{AgentSessionId, NodeIdPath, SessionId};
    use async_trait::async_trait;

    /// Ignores the queried prefix and always returns the full fixed row set —
    /// this is deliberate: it lets tests assert that `dispatch_list_threads`
    /// (via `aggregate_threads`'s `strip_prefix` check) is the thing doing the
    /// filtering, not a backend that happens to filter correctly itself.
    struct StubRepo {
        rows: Vec<NodeActivity>,
    }

    #[async_trait]
    impl ConversationRepository for StubRepo {
        async fn get_by_id(&self, _key: &ConversationKey) -> Result<Conversation, LlmError> {
            Ok(Conversation {
                key: ConversationKey {
                    session_id: SessionId("s".to_string()),
                    agent_session_id: None,
                    node_id: NodeIdPath("n".to_string()),
                },
                messages: vec![],
            })
        }
        async fn add_message(
            &self,
            _key: &ConversationKey,
            _message: LlmMessage,
        ) -> Result<(), LlmError> {
            Ok(())
        }
        async fn delete(&self, _key: &ConversationKey) -> Result<(), LlmError> {
            Ok(())
        }
        async fn list_node_activity(
            &self,
            _keying: (&str, &str),
            _node_id_prefix: &str,
        ) -> Result<Vec<NodeActivity>, LlmError> {
            Ok(self.rows.clone())
        }
    }

    fn key() -> ConversationKey {
        ConversationKey {
            session_id: SessionId("s".to_string()),
            agent_session_id: Some(AgentSessionId("agent_test".to_string())),
            node_id: NodeIdPath("n".to_string()),
        }
    }

    #[tokio::test]
    async fn dispatch_unknown_tool_returns_error_with_available_list() {
        let repo: Arc<dyn ConversationRepository> = Arc::new(StubRepo { rows: vec![] });
        let dynamic_tool_names = vec!["archivador".to_string(), "asesor".to_string()];
        let r = dispatch_list_threads(
            &repo,
            &key(),
            &dynamic_tool_names,
            serde_json::json!({"tool": "not_dynamic"}),
        )
        .await;
        assert_eq!(r["error"], "unknown_or_non_dynamic_tool: 'not_dynamic'");
        assert_eq!(
            r["available_dynamic_tools"],
            serde_json::json!(["archivador", "asesor"])
        );
    }

    #[tokio::test]
    async fn dispatch_excludes_rows_from_other_tools_prefix() {
        // StubRepo ignores the requested prefix and returns rows for BOTH
        // "archivador" and "asesor" on every call; only rows under the
        // requested tool's `tool/<name>/` prefix must survive into the
        // aggregated output for that tool.
        let rows = vec![
            na(
                "tool/archivador/alfa/keeper",
                4,
                "2026-08-24T10:00:00Z",
                "hola",
            ),
            na("tool/asesor/caso-12", 5, "2026-08-24T12:00:00Z", "otro"),
        ];
        let repo: Arc<dyn ConversationRepository> = Arc::new(StubRepo { rows });
        let dynamic_tool_names = vec!["archivador".to_string()];
        let r = dispatch_list_threads(
            &repo,
            &key(),
            &dynamic_tool_names,
            serde_json::json!({"tool": "archivador"}),
        )
        .await;
        let threads = r["tools"][0]["threads"].as_array().unwrap();
        assert_eq!(threads.len(), 1);
        assert_eq!(threads[0]["thread_id"], "alfa");
    }

    #[tokio::test]
    async fn dispatch_marks_truncated_when_rows_hit_the_cap() {
        let rows: Vec<NodeActivity> = (0..MAX_LISTED_NODE_ACTIVITY)
            .map(|i| {
                na(
                    &format!("tool/archivador/thread-{i}"),
                    1,
                    "2026-08-24T10:00:00Z",
                    "hola",
                )
            })
            .collect();
        let repo: Arc<dyn ConversationRepository> = Arc::new(StubRepo { rows });
        let dynamic_tool_names = vec!["archivador".to_string()];
        let r = dispatch_list_threads(
            &repo,
            &key(),
            &dynamic_tool_names,
            serde_json::json!({"tool": "archivador"}),
        )
        .await;
        assert_eq!(r["tools"][0]["truncated"], true);
    }

    #[tokio::test]
    async fn dispatch_omits_truncated_when_rows_under_the_cap() {
        let rows = vec![na(
            "tool/archivador/alfa/keeper",
            4,
            "2026-08-24T10:00:00Z",
            "hola",
        )];
        let repo: Arc<dyn ConversationRepository> = Arc::new(StubRepo { rows });
        let dynamic_tool_names = vec!["archivador".to_string()];
        let r = dispatch_list_threads(
            &repo,
            &key(),
            &dynamic_tool_names,
            serde_json::json!({"tool": "archivador"}),
        )
        .await;
        assert!(r["tools"][0].get("truncated").is_none());
    }
}
