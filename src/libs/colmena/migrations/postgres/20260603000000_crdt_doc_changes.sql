CREATE TABLE IF NOT EXISTS crdt_doc_events (
    id          BIGSERIAL   PRIMARY KEY,
    artifact_id TEXT        NOT NULL,
    sheet_id    TEXT,
    origin      TEXT        NOT NULL,
    summary     TEXT        NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS crdt_doc_events_lookup
    ON crdt_doc_events(artifact_id, id);

CREATE INDEX IF NOT EXISTS crdt_doc_events_by_sheet
    ON crdt_doc_events(artifact_id, sheet_id, id);

CREATE TABLE IF NOT EXISTS crdt_doc_session_cursors (
    agent_session_id TEXT        NOT NULL,
    artifact_id      TEXT        NOT NULL,
    last_event_id    BIGINT      NOT NULL,
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (agent_session_id, artifact_id)
);

CREATE TABLE IF NOT EXISTS crdt_doc_session_artifacts (
    agent_session_id TEXT        NOT NULL,
    artifact_id      TEXT        NOT NULL,
    name             TEXT        NOT NULL,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_accessed_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (agent_session_id, artifact_id)
);

CREATE INDEX IF NOT EXISTS crdt_doc_session_artifacts_recent_idx
    ON crdt_doc_session_artifacts(agent_session_id, last_accessed_at DESC);
