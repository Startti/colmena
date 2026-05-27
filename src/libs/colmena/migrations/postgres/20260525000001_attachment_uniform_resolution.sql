-- Attachment uniform resolution — Plan A
-- Add storage_key (reference to OutputStorageRepository), origin (semantic
-- source), and last_used_at (for TTL).
-- Migration is additive: all new columns nullable; existing rows unaffected.

ALTER TABLE conversation_attachments
  ADD COLUMN IF NOT EXISTS storage_key TEXT,
  ADD COLUMN IF NOT EXISTS origin TEXT,
  ADD COLUMN IF NOT EXISTS last_used_at TIMESTAMPTZ;

UPDATE conversation_attachments
SET origin = CASE
  WHEN provider = 'generated' THEN 'generated_by:unknown'
  ELSE 'user_upload'
END
WHERE origin IS NULL;

CREATE INDEX IF NOT EXISTS idx_conv_attachments_session_used
  ON conversation_attachments (agent_session_id, last_used_at);

-- Rollback:
-- DROP INDEX IF EXISTS idx_conv_attachments_session_used;
-- ALTER TABLE conversation_attachments
--   DROP COLUMN IF EXISTS last_used_at,
--   DROP COLUMN IF EXISTS origin,
--   DROP COLUMN IF EXISTS storage_key;
