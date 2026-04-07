CREATE TABLE IF NOT EXISTS dag_task_memory (
    id UUID PRIMARY KEY,
    session_id VARCHAR(255) NOT NULL,
    task_name TEXT NOT NULL,
    assigned_to VARCHAR(255) NOT NULL,
    completed BOOLEAN NOT NULL DEFAULT FALSE,
    result JSONB,
    phase INT NOT NULL DEFAULT 1,
    parallel BOOLEAN NOT NULL DEFAULT FALSE,
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