-- gdocs_session_state v1.1 extension — persist last DocumentSnapshot
-- so the co-edit guard can show paragraph-level diffs of human changes.

ALTER TABLE gdocs_session_state
  ADD COLUMN IF NOT EXISTS last_snapshot_json       JSONB,
  ADD COLUMN IF NOT EXISTS last_snapshot_size_bytes INTEGER;

-- Rollback:
-- ALTER TABLE gdocs_session_state DROP COLUMN IF EXISTS last_snapshot_size_bytes;
-- ALTER TABLE gdocs_session_state DROP COLUMN IF EXISTS last_snapshot_json;
