-- Adds chat-scoped identifier and per-node identifier to llm_node_history.
-- Pre-existing rows have node_id = NULL and are excluded from new reads.
-- See docs/superpowers/specs/2026-04-28-agent-session-id-design.md §3.2.

ALTER TABLE llm_node_history
    ADD COLUMN IF NOT EXISTS agent_session_id TEXT,
    ADD COLUMN IF NOT EXISTS node_id TEXT;

CREATE INDEX IF NOT EXISTS idx_llm_history_agent_node
    ON llm_node_history(agent_session_id, node_id, created_at);

CREATE INDEX IF NOT EXISTS idx_llm_history_session_node
    ON llm_node_history(session_id, node_id, created_at);
