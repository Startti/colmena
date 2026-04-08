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

CREATE TABLE IF NOT EXISTS dag_runs (
    session_id VARCHAR(255) PRIMARY KEY,
    graph_json JSONB NOT NULL,
    all_outputs JSONB NOT NULL,
    status VARCHAR(50) NOT NULL,
    created_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS dag_task_memory (
    id UUID PRIMARY KEY,
    session_id VARCHAR(255) NOT NULL,
    task_name TEXT NOT NULL,
    assigned_to VARCHAR(255) NOT NULL,
    completed BOOLEAN NOT NULL DEFAULT FALSE,
    result JSONB,
    phase INT NOT NULL DEFAULT 1,
    parallel BOOLEAN NOT NULL DEFAULT FALSE,
    is_bridge BOOLEAN NOT NULL DEFAULT FALSE,
    context TEXT,
    created_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_dag_task_memory_session_id ON dag_task_memory(session_id);
CREATE INDEX IF NOT EXISTS idx_dag_task_memory_phase ON dag_task_memory(session_id, phase, completed);

CREATE TABLE IF NOT EXISTS dag_phase_summaries (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    session_id TEXT NOT NULL,
    phase INT NOT NULL,
    summary TEXT NOT NULL,
    created_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_dag_phase_summaries_session_id ON dag_phase_summaries(session_id);
