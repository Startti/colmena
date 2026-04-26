-- Adds the 5 JSONB execution-state columns that dag_runs was missing from the
-- initial schema. All statements use ADD COLUMN IF NOT EXISTS so this migration
-- is safe to apply against databases that already have these columns.
ALTER TABLE dag_runs ADD COLUMN IF NOT EXISTS active_queue          JSONB NOT NULL DEFAULT '[]'::jsonb;
ALTER TABLE dag_runs ADD COLUMN IF NOT EXISTS execution_history     JSONB NOT NULL DEFAULT '[]'::jsonb;
ALTER TABLE dag_runs ADD COLUMN IF NOT EXISTS global_calls          JSONB NOT NULL DEFAULT '{}'::jsonb;
ALTER TABLE dag_runs ADD COLUMN IF NOT EXISTS caller_specific_calls JSONB NOT NULL DEFAULT '{}'::jsonb;
ALTER TABLE dag_runs ADD COLUMN IF NOT EXISTS global_shared_state   JSONB NOT NULL DEFAULT '{}'::jsonb;
