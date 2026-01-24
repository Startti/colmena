# ⚡ EPIC 2: Real-time Streaming

**Objetivo**: Habilitar SSE para visualización de tokens en vivo.
**Dependencias**: EPIC 1 completado.

---

## 📋 Lista de Tareas

### STREAM-01: Worker Pub/Sub Emitter
**Objetivo**: Que el worker "grite" los eventos importantes a Redis.

1.  **ExecutionObserver Trait (`src/libs/dag_engine/domain/observer.rs`)**:
    *   Definir trait:
    ```rust
    pub trait ExecutionObserver: Send + Sync {
        fn on_node_event(&self, node_id: &str, event_type: &str, payload: Value);
    }
    ```
    *   Integrar en `DagRunUseCase`: aceptar `Option<Arc<dyn ExecutionObserver>>` en el método `execute`.

2.  **RedisObserver Implementation (`src/platform/worker/src/observer.rs`)**:
    *   Implementar el trait.
    *   En `on_node_event`:
        *   Construir mensaje JSON: `{ "node_id": "...", "type": "token", "payload": "..." }`.
        *   Ejecutar `PUBLISH events:{job_id} <json>`.

3.  **LLM Node Integration**:
    *   En `src/libs/dag_engine/infrastructure/nodes/llm.rs`:
    *   Si hay un observer presente, llamar `observer.on_node_event()` por cada token recibido del stream del proveedor LLM.

### STREAM-02: API SSE Endpoint
**Objetivo**: Endpoint que mantiene conexión abierta con el cliente y le empuja datos.

1.  **Axum SSE Handler (`src/platform/api/src/stream.rs`)**:
    *   Endpoint `GET /api/v1/stream/:job_id`.
    *   Retorno: `Sse<impl Stream<Item = Result<Event, Infallible>>>`.

2.  **Redis Subscription**:
    *   Usar `redis::aio::PubSub`.
    *   Suscribirse al canal `events:{job_id}`.

3.  **Stream Logic**:
    *   Crear un `async_stream::stream!` block.
    *   Loop infinito leyendo mensajes de Redis.
    *   Por cada mensaje: `yield Event::default().data(msg_payload)`.
    *   Manejar desconexión del cliente (romper el loop).

4.  **Keep-Alive**:
    *   Configurar `Sse::new(stream).keep_alive(KeepAlive::default())` para evitar timeouts de proxies/load balancers.

### STREAM-03: Frontend Client (Proof of Concept)
**Objetivo**: Verificar visualmente que el streaming funciona.

1.  **HTML Simple (`test_stream.html`)**:
    ```html
    <script>
        const jobId = "job_123"; // ID obtenido del POST /executions
        const evtSource = new EventSource(`http://localhost:3000/api/v1/stream/${jobId}`);
        
        evtSource.onmessage = function(event) {
            const data = JSON.parse(event.data);
            if (data.type === 'token') {
                document.body.innerHTML += data.payload;
            }
        };
    </script>
    ```
