-- 2026-05-11 — sliding TTL: extend default expires_at to 24h.
-- Existing rows keep their original TTL and will be swept naturally by
-- cleanup_expired_for_run as their owning runs complete.
ALTER TABLE secure_value_mappings
    ALTER COLUMN expires_at SET DEFAULT NOW() + INTERVAL '24 hours';
