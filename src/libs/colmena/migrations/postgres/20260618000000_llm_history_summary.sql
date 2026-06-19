-- Cache de resumen semántico por mensaje (Fase 3, conversation summary).
-- NULL = aún no resumido (o < umbral → verbatim). Ver spec 2026-06-18.
ALTER TABLE llm_node_history ADD COLUMN IF NOT EXISTS summary TEXT;
