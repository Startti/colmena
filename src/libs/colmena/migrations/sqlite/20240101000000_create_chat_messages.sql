DROP TABLE IF EXISTS chat_messages;

CREATE TABLE IF NOT EXISTS llm_node_history (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    role TEXT NOT NULL,
    content TEXT NOT NULL,
    tool_call_id TEXT,
    tool_calls TEXT,
    created_at TEXT NOT NULL
);

CREATE INDEX idx_llm_node_history_session_id ON llm_node_history(session_id);
CREATE INDEX idx_llm_node_history_created_at ON llm_node_history(created_at);
