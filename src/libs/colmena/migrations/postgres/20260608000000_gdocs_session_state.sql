-- gdocs_session_state — last-known revision per (agent_session, doc).
-- Used by the co-edit guard to detect human edits between agent writes.

CREATE TABLE IF NOT EXISTS gdocs_session_state (
    agent_session_id TEXT        NOT NULL,
    document_id      TEXT        NOT NULL,
    last_revision_id TEXT        NOT NULL,
    last_edit_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (agent_session_id, document_id)
);

CREATE INDEX IF NOT EXISTS gdocs_session_state_last_edit_at_idx
    ON gdocs_session_state (last_edit_at);

-- Rollback:
-- DROP INDEX IF EXISTS gdocs_session_state_last_edit_at_idx;
-- DROP TABLE IF EXISTS gdocs_session_state;
