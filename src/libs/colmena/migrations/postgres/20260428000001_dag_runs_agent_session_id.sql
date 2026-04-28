-- Adds chat-scoped identifier and explicit parent linkage to dag_runs.
-- See docs/superpowers/specs/2026-04-28-agent-session-id-design.md §3.1.

ALTER TABLE dag_runs
    ADD COLUMN IF NOT EXISTS agent_session_id VARCHAR(255),
    ADD COLUMN IF NOT EXISTS parent_session_id VARCHAR(255);

CREATE INDEX IF NOT EXISTS idx_dag_runs_agent_session_id
    ON dag_runs(agent_session_id);

CREATE INDEX IF NOT EXISTS idx_dag_runs_parent_session_id
    ON dag_runs(parent_session_id);

CREATE INDEX IF NOT EXISTS idx_dag_runs_agent_status
    ON dag_runs(agent_session_id, status);
