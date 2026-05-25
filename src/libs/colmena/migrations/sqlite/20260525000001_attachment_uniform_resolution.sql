-- Attachment uniform resolution — Plan A (SQLite).
-- Add storage_key (reference to OutputStorageRepository), origin (semantic
-- source), and last_used_at (for TTL).
-- All columns nullable so the migration is additive on existing rows.

ALTER TABLE conversation_attachments ADD COLUMN storage_key TEXT;
ALTER TABLE conversation_attachments ADD COLUMN origin TEXT;
ALTER TABLE conversation_attachments ADD COLUMN last_used_at TEXT;

-- Best-effort origin backfill for legacy rows: anything from `generated`
-- becomes `generated_by:unknown`, the rest is treated as a user upload.
UPDATE conversation_attachments
SET origin = CASE
  WHEN provider = 'generated' THEN 'generated_by:unknown'
  ELSE 'user_upload'
END
WHERE origin IS NULL;

CREATE INDEX IF NOT EXISTS idx_conv_attachments_session_used
  ON conversation_attachments (agent_session_id, last_used_at);
