-- Enables pgcrypto for AES-256 encryption and creates the secure_value_mappings
-- table used by SecureValue nodes to store encrypted secrets with session scope
-- and a 1-hour TTL.
CREATE EXTENSION IF NOT EXISTS pgcrypto;

CREATE TABLE IF NOT EXISTS secure_value_mappings (
    id              UUID         PRIMARY KEY DEFAULT gen_random_uuid(),
    session_id      VARCHAR(255) NOT NULL,
    source_node_id  VARCHAR(255) NOT NULL,
    hash_key        VARCHAR(255) NOT NULL,
    encrypted_value BYTEA        NOT NULL,
    field_name      VARCHAR(255),
    created_at      TIMESTAMPTZ  DEFAULT NOW(),
    expires_at      TIMESTAMPTZ  DEFAULT (NOW() + INTERVAL '1 hour'),
    UNIQUE(session_id, hash_key)
);

CREATE INDEX IF NOT EXISTS idx_secure_session_id ON secure_value_mappings(session_id);
CREATE INDEX IF NOT EXISTS idx_secure_hash_key   ON secure_value_mappings(session_id, hash_key);
CREATE INDEX IF NOT EXISTS idx_secure_expires_at ON secure_value_mappings(expires_at);
