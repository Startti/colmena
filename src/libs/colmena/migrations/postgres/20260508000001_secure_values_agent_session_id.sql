-- Spec: docs/superpowers/specs/2026-05-08-secure-values-agent-session-id-design.md
-- Adds stable-scope identifier so secure values can be looked up across runs
-- that share an agent context (canvas-builder pattern).

ALTER TABLE secure_value_mappings
    ADD COLUMN IF NOT EXISTS agent_session_id TEXT;

CREATE INDEX IF NOT EXISTS idx_secure_values_agent_hash
    ON secure_value_mappings(agent_session_id, hash_key);
