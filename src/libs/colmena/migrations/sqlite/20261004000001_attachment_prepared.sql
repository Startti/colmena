-- Preparation registry for large tabular attachments (dark behind
-- COLMENA_LARGE_TABULAR). One row per source storage key; a row starts at
-- `running` when a worker claims it and ends `ready` or `failed`. `status` is
-- plain TEXT so a `pending` state can be added later without a migration of
-- existing rows. Timestamps are written by the application (the claim rule
-- compares them with a caller-supplied "now"), never by column defaults.
CREATE TABLE IF NOT EXISTS attachment_prepared (
    source_storage_key TEXT PRIMARY KEY,
    status             TEXT NOT NULL,
    format_version     INTEGER NOT NULL,
    manifest_key       TEXT,
    blob_keys          TEXT NOT NULL DEFAULT '[]',
    tables_json        TEXT,
    source_bytes       BIGINT NOT NULL,
    prepared_bytes     BIGINT,
    error_code         TEXT,
    error_detail       TEXT,
    lease_owner        TEXT,
    lease_until        TIMESTAMP,
    attempts           INTEGER NOT NULL DEFAULT 0,
    created_at         TIMESTAMP NOT NULL,
    updated_at         TIMESTAMP NOT NULL,
    last_used_at       TIMESTAMP
);

-- Rollback:
-- DROP TABLE IF EXISTS attachment_prepared;
