CREATE TABLE IF NOT EXISTS dag_task_memory (
    id UUID PRIMARY KEY,
    session_id VARCHAR(255) NOT NULL,
    task_name VARCHAR(255) NOT NULL,
    assigned_to VARCHAR(255) NOT NULL,
    completed BOOLEAN NOT NULL DEFAULT FALSE,
    result JSONB,
    created_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_dag_task_memory_session_id ON dag_task_memory(session_id);
