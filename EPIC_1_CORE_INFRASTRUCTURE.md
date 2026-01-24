# 🏗️ EPIC 1: Core Platform Infrastructure

**Objetivo**: Establecer la arquitectura base (API Producer + Worker Consumer) y lograr la ejecución "Hello World" de un DAG distribuido.
**Semanas Estimadas**: 1-2

---

## 📋 Lista de Tareas

### CORE-01: Setup Monorepo Structure & Workspace
**Objetivo**: Reorganizar el proyecto para soportar múltiples binarios compartiendo la librería `dag_engine`.

1.  **Reorganización de Directorios**:
    *   Crear directorio `src/libs`.
    *   Mover `src/dag_engine` (y todo su contenido) a `src/libs/dag_engine`.
    *   Actualizar `src/libs/dag_engine/Cargo.toml` para que el `name` sea `colmena_dag_engine`.

2.  **Creación de Nuevos Servicios**:
    *   `src/platform/api`: `cargo new src/platform/api --bin`
        *   Dependencias iniciales: `axum`, `tokio`, `serde`, `serde_json`, `tower-http`, `tracing`, `tracing-subscriber`.
    *   `src/platform/worker`: `cargo new src/platform/worker --bin`
        *   Dependencias iniciales: `tokio`, `serde`, `serde_json`, `redis`, `tracing`, `tracing-subscriber`.
    *   `src/platform/shared`: `cargo new src/platform/shared --lib`
        *   Dependencias iniciales: `serde`, `serde_json`.

3.  **Configuración del Workspace (`Cargo.toml` raíz)**:
    ```toml
    [workspace]
    members = [
        "src/libs/dag_engine",
        "src/platform/api",
        "src/platform/worker",
        "src/platform/shared"
    ]
    resolver = "2"
    ```

### CORE-02: Deploy Redis Infrastructure
**Objetivo**: Levantar la infraestructura de mensajería necesaria para la cola de trabajos.

1.  **Docker Compose**:
    *   Crear/Actualizar `docker-compose.yml` en la raíz.
    ```yaml
    services:
      redis:
        image: redis:7-alpine
        ports:
          - "6379:6379"
        volumes:
          - redis_data:/data
        command: redis-server --appendonly yes  # Persistencia activada
    
    volumes:
      redis_data:
    ```

2.  **Config Management**:
    *   En `src/platform/shared/src/config.rs`:
        *   Helper function `get_redis_url() -> String` que lea `REDIS_URL` del environment o use `redis://localhost:6379` por defecto.

### CORE-03: Implement Job Protocol
**Objetivo**: Definir el "contrato" de datos entre la API y el Worker.

1.  **JobRequest Struct (`src/platform/shared/src/lib.rs`)**:
    ```rust
    use serde::{Deserialize, Serialize};
    use serde_json::Value;

    #[derive(Serialize, Deserialize, Debug, Clone)]
    pub struct JobRequest {
        pub job_id: String,      // UUID v4 único para esta ejecución
        pub dag_json: Value,     // El grafo completo a ejecutar
        pub inputs: Value,       // Inputs iniciales para el DAG
        pub created_at: i64,     // Timestamp (Unix ms)
    }
    ```

2.  **JobStatus Enum (Opcional por ahora, útil futuro)**:
    ```rust
    #[derive(Serialize, Deserialize, Debug)]
    pub enum JobStatus {
        Queued,
        Running,
        Completed,
        Failed(String),
    }
    ```

### CORE-04: API Enqueue Endpoint
**Objetivo**: Endpoint HTTP para recibir y encolar trabajos.

1.  **Axum Handler**:
    *   Archivo: `src/platform/api/src/handlers.rs`
    *   Endpoint: `POST /api/v1/executions`
    *   Body: JSON con `{ "dag_json": {...}, "inputs": {...} }` (Map to a DTO, then to `JobRequest`).
    
2.  **Lógica**:
    *   Generar `job_id = Uuid::new_v4().to_string()`.
    *   Crear instancia de `JobRequest`.
    *   Serializar a String JSON.
    *   Obtener conexión Redis (usando pool `deadpool-redis` o client simple).
    *   Ejecutar comando `LPUSH job_queue <json_string>`.
    *   Retornar `202 Accepted` JSON: `{ "job_id": "...", "status": "queued" }`.

3.  **Error Handling**:
    *   Si Redis falla -> `503 Service Unavailable`.
    *   Si JSON inválido -> `400 Bad Request`.

### CORE-05: Worker Consumer Loop
**Objetivo**: El ciclo de vida principal del Worker.

1.  **Conexión Redis**:
    *   El worker debe mantener una conexión abierta (o un pool).

2.  **Loop Principal (`src/platform/worker/src/main.rs`)**:
    ```rust
    loop {
        // BRPOP bloquea hasta que haya un elementos en 'job_queue'. Timeout 0 = infinito.
        let result: Option<(String, String)> = con.brpop("job_queue", 0).await?;
        
        if let Some((_list, job_json)) = result {
            // Deserializar
            let job: JobRequest = serde_json::from_str(&job_json)?;
            
            info!("Processing Job: {}", job.job_id);
            
            // Llamar a la función de ejecución (CORE-06)
            process_job(job).await?;
        }
    }
    ```

3.  **Signal Handling**:
    *   Usar `tokio::signal::ctrl_c` para un shutdown limpio. Si se recibe señal, terminar el trabajo actual antes de salir.

### CORE-06: Worker DAG Execution
**Objetivo**: Integrar la librería `dag_engine` para ejecutar el trabajo real.

1.  **Importar Librería**:
    *   En `src/platform/worker/Cargo.toml`:
        `colmena_dag_engine = { path = "../../libs/dag_engine" }`

2.  **Función `process_job`**:
    *   Instanciar el registro de nodos: `let registry = HashMapNodeRegistry::new();`
    *   Instanciar Use Case: `let use_case = DagRunUseCase::new(registry);`
    *   Convertir `job.dag_json` a struct `Graph` (usar `serde_json::from_value`).
    *   Ejecutar: `let result = use_case.execute(graph).await;`
    
3.  **Logging de Resultados**:
    *   `Ok(res)` -> `info!("Job {} finished successfully: {:?}", job.job_id, res);`
    *   `Err(e)` -> `error!("Job {} failed: {:?}", job.job_id, e);`
