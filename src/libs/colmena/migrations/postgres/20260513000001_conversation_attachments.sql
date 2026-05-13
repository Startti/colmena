CREATE TABLE IF NOT EXISTS conversation_attachments (
    agent_session_id  TEXT        NOT NULL,
    document_id       TEXT        NOT NULL,
    provider          TEXT        NOT NULL,
    provider_file_id  TEXT        NOT NULL,
    mime_type         TEXT        NOT NULL,
    filename          TEXT        NOT NULL,
    size_bytes        BIGINT,
    label             TEXT,
    description       TEXT,
    source_kind       TEXT        NOT NULL,
    source_value      TEXT,
    registered_at     TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    refreshed_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (agent_session_id, document_id, provider)
);

CREATE INDEX IF NOT EXISTS idx_conversation_attachments_session
    ON conversation_attachments(agent_session_id);
