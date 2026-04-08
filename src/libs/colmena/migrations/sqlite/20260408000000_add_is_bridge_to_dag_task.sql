-- Add missing columns from postgres schema (phase, parallel, context) if they don't exist yet,
-- and add the new is_bridge column. SQLite does not support ADD COLUMN IF NOT EXISTS,
-- so these are safe no-op migrations only when the column is truly missing.
ALTER TABLE dag_task_memory ADD COLUMN phase INTEGER NOT NULL DEFAULT 1;
ALTER TABLE dag_task_memory ADD COLUMN parallel BOOLEAN NOT NULL DEFAULT 0;
ALTER TABLE dag_task_memory ADD COLUMN context TEXT;
ALTER TABLE dag_task_memory ADD COLUMN is_bridge BOOLEAN NOT NULL DEFAULT 0;
