CREATE TABLE IF NOT EXISTS dag_task_memory (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    task_name TEXT NOT NULL,
    assigned_to TEXT NOT NULL,
    completed BOOLEAN NOT NULL DEFAULT FALSE,
    result TEXT,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_dag_task_memory_session_id ON dag_task_memory(session_id);
