# 🛡️ EPIC 4: Security Hardening

**Objetivo**: Proteger la plataforma de código malicioso mediante aislamiento de Python.

---

## 📋 Lista de Tareas

### SEC-01: DAG Inspector Service
**Objetivo**: Detectar DAGs peligrosos antes de ejecutarlos en el worker principal.

1.  **Inspector Module (`src/platform/worker/src/security.rs`)**:
    *   Función `inspect_dag(dag_json: &Value) -> SecurityReport`.
    *   Recorrer recursivamente `nodes`.
    *   Verificar `type` de cada nodo contra una `ALLOWLIST` (http, llm, math).
    *   Si encuentra `python_script`: `report.requires_isolation = true`.

2.  **Routing Logic**:
    *   En `process_job`:
        *   `let report = inspect_dag(&job.dag_json);`
        *   Si `requires_isolation` es true y no tenemos Sandbox configurado -> REJECT Job.
        *   (Futuro) Si `requires_isolation`, enviar a cola `job_queue_isolated`.

### SEC-02: Isolated Sandbox Runner (Prototype)
**Objetivo**: Ejecutar Python fuera del proceso del Worker.

1.  **Python Microservice**:
    *   Crear `src/platform/sandbox/app.py`.
    *   Framework: FastAPI/Flask.
    *   Endpoint: `POST /execute`.
    *   Body: `{ "code": "...", "inputs": {...} }`.
    *   Logic:
        ```python
        exec(code, inputs) # En un proceso separado o con restricciones
        return {"output": inputs.get("output")}
        ```

2.  **Dockerization**:
    *   `Dockerfile` para el sandbox.
    *   Sin acceso a red (o limitado).
    *   Usuario no-root.

### SEC-03: Remote Executor Node Adapter
**Objetivo**: Que el `PythonNode` de Rust sepa delegar.

1.  **Config Switch**:
    *   Añadir `PYTHON_EXECUTION_MODE` env var (`local` | `remote`).

2.  **Remote Logic**:
    *   En `src/libs/dag_engine/infrastructure/nodes/python_node.rs`:
    *   Si modo es `remote`:
        *   No usar `spawn_blocking` + `pyo3`.
        *   Hacer HTTP POST al servicio Sandbox (`http://sandbox-service:8000/execute`).
        *   Deserializar la respuesta JSON.
