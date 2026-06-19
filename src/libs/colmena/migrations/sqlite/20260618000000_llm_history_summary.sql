-- Cache de resumen semántico por mensaje (Fase 3, conversation summary).
-- SQLite no soporta IF NOT EXISTS en ADD COLUMN; la migración corre una sola vez.
ALTER TABLE llm_node_history ADD COLUMN summary TEXT;
