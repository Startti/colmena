DROP TABLE IF EXISTS chat_messages;

CREATE TABLE IF NOT EXISTS llm_node_history (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    session_id TEXT NOT NULL,
    role TEXT NOT NULL,
    content TEXT NOT NULL,
    tool_call_id TEXT,
    tool_calls JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_llm_node_history_session_id ON llm_node_history(session_id);
CREATE INDEX IF NOT EXISTS idx_llm_node_history_created_at ON llm_node_history(created_at);
