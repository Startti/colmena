-- Cache persistido de archivos subidos al Files API de cada proveedor LLM.
-- Permite reutilizar uploads entre ejecuciones de DAG dentro de la misma
-- conversación (Colmena es stateless por ejecución).
-- Ver docs/superpowers/specs/2026-05-02-large-document-files-api-design.md.

CREATE TABLE IF NOT EXISTS provider_file_cache (
    document_id      TEXT        NOT NULL,
    provider         TEXT        NOT NULL,
    provider_file_id TEXT        NOT NULL,
    mime_type        TEXT        NOT NULL,
    filename         TEXT        NOT NULL,
    size_bytes       BIGINT,
    uploaded_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at       TIMESTAMPTZ,
    last_used_at     TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (document_id, provider)
);

CREATE INDEX IF NOT EXISTS idx_provider_file_cache_expires
    ON provider_file_cache (expires_at)
    WHERE expires_at IS NOT NULL;
