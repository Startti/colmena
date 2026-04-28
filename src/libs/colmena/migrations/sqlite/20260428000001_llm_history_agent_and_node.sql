-- SQLite mirror of the Postgres llm_history migration (§3.2 of the spec).
-- Adds agent_session_id and node_id columns to llm_node_history table.

ALTER TABLE llm_node_history ADD COLUMN agent_session_id TEXT;
ALTER TABLE llm_node_history ADD COLUMN node_id TEXT;

CREATE INDEX IF NOT EXISTS idx_llm_history_agent_node
    ON llm_node_history(agent_session_id, node_id, created_at);

CREATE INDEX IF NOT EXISTS idx_llm_history_session_node
    ON llm_node_history(session_id, node_id, created_at);
