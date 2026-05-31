# Colmena DAG Engine: Technical Reference Guide

Este documento proporciona una explicación detallada de la arquitectura del repo Colmena, su motor de ejecución DAG y el catálogo de pruebas verificadas.

---

## 1. Estructura del Repositorio

El proyecto está organizado siguiendo principios de **Arquitectura Limpia**, separando las definiciones de datos de la implementación técnica.

### Directorios Principales
- `src/libs/colmena/src/dag_engine`: Núcleo del motor de grafos.
    - `domain/`: Modelos base (`Graph`, `Node`, `Edge`). Define la **identidad unificada** (`SessionId`).
    - `application/`: Orquestador (`run_use_case.rs`). Gestiona la propagación del `session_id`.
    - `infrastructure/`: Implementaciones concretas.
        - `nodes/`: Catálogo de todos los tipos de nodos (Python, LLM, Math, etc.).
        - `persistence/`: Gestión de la base de datos Postgres y el estado de ejecución.
        - `registry.rs`: Registro dinámico de nodos.
- `src/libs/colmena/src/llm`: Infraestructura para llamadas a modelos de lenguaje.
- `tests/graphs/`: Repositorio de ejemplos de grafos organizados por categorías (`basic`, `agents`, `advanced`, etc.).

---

## 2. Funcionamiento del Motor (The Engine)

El corazón de Colmena es el `DagRunUseCase`. Su función principal es transformar un archivo JSON en una secuencia coordinada de ejecuciones.

### El Ciclo de Ejecución (`execute_stream`)
Cuando se inicia un grafo, el motor sigue estos pasos:
1.  **Carga de Estado**: Si se pasa un `resume_id`, recupera el estado anterior de Postgres. Si no, inicializa una cola con los nodos que no tienen dependencias de entrada.
2.  **Bucle de Eventos**: Mientras haya nodos en la cola (`active_queue`):
    - **Validación de Entradas**: Verifica si todos los nodos predecesores han terminado y entregado sus datos.
    - **Construcción de Inputs**: Recolecta los datos de salida de los nodos padres usando **JSON Pointers** definidos en los `edges`.
    - **Ejecución de Nodo**: Llama al método `execute` del nodo correspondiente.
    - **Gestión de Salida**: Guarda el resultado y decide qué nodos hijos activar basándose en las reglas de los `edges`.
3.  **Finalización**: Emite un evento `finish` con el resultado agregado de todos los nodos.

### Mecanismo de Suspensión y Resume
Es una de las características más avanzadas de Colmena:
- **Suspensión**: Si un nodo devuelve la bandera `__colmena_status: "SUSPENDED"`, el motor pausa todo, guarda la cola de ejecución actual y las variables en Postgres bajo el `session_id` generado.
- **Reanudación**: Al usar `--session-id` (o `--resume-id`), el motor restaura el estado exacto. El nodo recibe la respuesta en `__colmena_resume_answer`.

---

## 3. Catálogo de Grafos Verificados (`basic/`)

A continuación se explica el comportamiento de los grafos verificados en la carpeta `tests/graphs/basic/`.

### 1. `power.json`
- **Propósito**: Demostrar operaciones matemáticas encadenadas con auto-aplanado.
- **Flujo**:
    1.  `input`: Define un número base (ej: 5).
    2.  `pow_step`: Eleva el número al cubo (5^3).
    3.  `final_log`: Muestra el resultado.
- **Respuesta**: Devuelve el resultado matemático directo.

### 2. `power_webhook.json`
- **Propósito**: Ejecución iniciada por un evento externo (Webhook).
- **Flujo**:
    1.  `trigger`: Simula la recepción de un JSON externo con un campo `input`.
    2.  `pow_step`: Procesa el `input` del webhook.
- **Clave**: El motor inyecta el payload del webhook directamente en el flujo.

### 3. `python_simple_graph.json`
- **Propósito**: Ejecución de código arbitrario usando Python.
- **Flujo**: Toma dos números (`a` y `b`), los suma en un script de Python y devuelve el `output`.
- **Respuesta**: Un JSON con el resultado del script.

### 4. `test_cyclic_graph.json`
- **Propósito**: Demostrar que el motor puede manejar bucles cerrados (ciclos).
- **Mecánica**: El borde que vuelve atrás debe estar marcado con `"cyclic": true`. Esto evita que el motor se quede esperando infinitamente un dato que aún no se ha generado en la primera vuelta.

### 5. `test_cyclic_early_stop.json`
- **Propósito**: Finalización condicional de un bucle.
- **Comportamiento**: Un script de Python evalúa una condición. Si decide que el bucle debe terminar, devuelve `null`. El motor detecta el `null` y deja de propagar la ejecución por esa rama, rompiendo el ciclo de forma limpia.

### 7. `trigger.json`
- **Propósito**: Ejecución simple iniciada por Webhook con simulación (`test_payload`).
- **Respuesta**: El nodo `my_webhook` devuelve el objeto configurado en `test_payload`. El nodo `log_step` lo imprime.
- **Resultado Final**: Un mapa con los datos de ambos nodos.

### 8. `input_example.json`
- **Propósito**: El ejemplo más básico de flujo de datos.
- **Respuesta**: El nodo `start` emite una pregunta y un contexto. El motor los inyecta en `log_result`.
- **Dato clave**: Aquí se observa el resultado del "Output Flattening"; el nodo `input` devuelve sus campos directamente sin envoltorios.

### 9. `test_loop.json`
- **Propósito**: Demostrar la lógica de control de bucles.
- **Mecánica**: 
    - No tiene un borde cíclico explícito, por lo que en modo `single-turn` solo se ejecuta una vez.
    - Se basa en la variable `__colmena_loop_status`. Si es `"NEXT_TURN"`, indica que el nodo desea seguir iterando.
    - Para que este grafo sea útil en producción, se suele ejecutar con el flag `--loop` o dentro de un orquestador que maneje ciclos.

---

## 3b. Catálogo de Grafos HTTP (`external/`)

Los ejemplos de peticiones HTTP viven en `tests/graphs/external/`, no en `basic/`.

### `external/http_request.json`
- **Propósito**: Realizar peticiones HTTP salientes.
- **Flujo**: Consulta una API pública de chistes y muestra el resultado.
- **Clave**: El nodo `http_request` mapea automáticamente el cuerpo de la respuesta a la llave `body`.

### `external/dynamic_http.json`
- **Propósito**: Inyección dinámica de parámetros en peticiones HTTP.
- **Flujo**: Un webhook envía la ruta del endpoint (ej: `/random_joke`) y el nodo HTTP la concatena a su `base_url`.
- **Clave**: Demuestra que casi cualquier campo de la `config` de los nodos puede ser sobrescrito por `inputs` dinámicos.

### `external/http_tool_configured.json`
- **Propósito**: Uso de nodos HTTP como "Tools" para un Agente LLM.
- **Mecánica**: Un nodo `llm_call` tiene configuradas herramientas que, por debajo, ejecutan nodos `http_request` pre-configurados.
- **Resultado**: El agente decide qué herramienta llamar, el motor ejecuta la petición HTTP, y el resultado vuelve al agente para que genere una respuesta en lenguaje natural.

### `external/http_headers_dynamic.json`
- **Propósito**: Construcción dinámica de headers HTTP a partir de inputs del grafo.

Otros catálogos relacionados:
- `tests/graphs/web/`: integraciones con APIs externas vía OpenAPI (`api_explorer_*.json`) y búsqueda web Tavily (`tavily_*.json`).
- `tests/graphs/security/http_secure_*.json`: peticiones HTTP con `secure_value_mappings` (TTL 24h) para inyectar credenciales sin filtrarlas a logs.
- `tests/graphs/agents/`: agentes LLM que usan `http_tool` como herramientas (`agent_http_tool_*.json`, `http_tool_node_schema_test.json`).

---

## 4. Conceptos Avanzados de Datos

### Auto-Flattening (Aplanado Automático)
Si conectas el nodo A al B sin especificar rutas exactas (`"from": "A", "to": "B"`), el motor inyecta todas las llaves de la salida de A como variables individuales en B. Esto permite crear grafos muy rápidos de "prototipar".

### JSON Pointers
Puedes extraer datos muy profundos: `"from": "llm.choices.0.message.content"`. El motor navegará el JSON de salida automáticamente para encontrar ese valor exacto.

## 5. Glosario de Partes del Código

- **`all_outputs`**: Un Mapa (HashMap) que guarda lo último que emitió cada nodo. Es la fuente de verdad para llenar los siguientes nodos.
- **`active_queue`**: La lista de tareas pendientes. Es dinámica; los nodos se meten o sacan de aquí según los datos que fluyen.
- **`build_inputs_for`**: La función más importante de `run_use_case.rs`. Es la que hace la "magia" de buscar datos en `all_outputs` basándose en las rutas de los `edges`.
- **`PostgresDagStateRepository`**: Nuestra capa de persistencia. Recientemente mejorada para guardar no solo los resultados, sino también la cola de ejecución y el historial de llamadas, permitiendo reanudar grafos complejos sin perder el progreso.
- **`Registry`**: El lugar donde se mapean nombres como `"python_script"` a la clase Rust que lo ejecuta. Permite que el sistema sea extensible sin tocar el núcleo del motor.

---

## 6. Observabilidad y Métricas

El motor de Colmena rastrea el uso de recursos a lo largo de toda la vida del grafo.

### Rastreo de Tokens (LLM Usage)
Una de las métricas más importantes es el uso de tokens. El sistema funciona de forma acumulativa:
1.  **Emisión**: Cada nodo que interactúa con un LLM emite eventos `LlmUsage` con los tokens de su llamada específica.
2.  **Agregación**: El orquestador de la CLI (`main.rs`) captura estos eventos y los suma en acumuladores globales.
3.  **Reporte Final**: Al emitir el evento `finish`, el motor incluye el objeto `usage` con la suma total de `promptTokens` y `completionTokens` de todos los nodos que participaron.

---

## 7. Soporte Multimedia (Visión y Documentos)

El motor permite enviar archivos a los modelos de lenguaje (especialmente útil para modelos con visión como GPT-4o o Gemini Flash).

### Cómo funciona
1.  **Configuración en el JSON**: En el nodo `llm_call`, se añade una lista de `files` con su `mime_type` y `path`.
2.  **Resolución de Archivos**: El motor lee el archivo del sistema local, lo codifica en Base64 y lo inserta en la estructura de mensajes del adaptador LLM.
3.  **Mime Types Soportados**:
    - `image/png`, `image/jpeg`: Soportados nativamente para visión.
    - `application/pdf`: Soportado para análisis de documentos (vía GPT-4o o Gemini).

**Ejemplos Verificados:**
- `media/image_path.json`: Envía una imagen local para descripción.
- `media/pdf_path.json`: Envía un PDF (ej. un poema) para análisis de texto.

---

## 8. Memoria y Persistencia de Conversación

El motor permite que los nodos LLM mantengan el contexto de una conversación a través de múltiples ejecuciones o pasos del grafo mediante un `thread_id`.

### Unificación de Identidad (session_id)
El motor utiliza un único identificador para toda la vida del grafo:
1.  **session_id**: Se genera al inicio (o se provee externamente).
2.  **Persistencia**: Se usa como llave primaria en `dag_runs` y como identificador de conversación en `llm_node_history`.
3.  **Transparencia**: Los nodos AI ya no requieren configurar un `thread_id` manual; heredan automáticamente el `session_id` de la ejecución actual.

### Cómo funciona
1.  **Session Context**: Al iniciar, el motor inyecta `session_id` en el estado global.
2.  **Repositorios de Memoria**: Soporta múltiples backends:
    - **Postgres**: Ideal para producción y escalabilidad. Se configura con un `connection_url` de PostgreSQL.
    - **SQLite**: Ideal para desarrollo local o sistemas embebidos.
    - **In-Memory**: (Default) Memoria efímera para la sesión actual.

**Ejemplos Verificados:**
- `memory/memory_postgres_example.json`: Un grafo de dos pasos donde el segundo paso recuerda datos del primero usando PostgreSQL.
- Más ejemplos: `tests/graphs/memory/`.

---

## 9. Módulos Documentados por Separado

Estas capacidades del motor están cubiertas en detalle en sus propios documentos. Aquí solo se listan como referencia rápida.

### Skills
Sistema de "skills" cargables que un agente LLM puede descubrir y leer en tiempo de ejecución.
- Configuración a nivel motor: `{ "builtin": ["..."], "paths": ["./ruta/al/skill_dir"] }` (parser en `src/libs/colmena/src/skills/domain/skill_config.rs`).
- También se pueden adjuntar por nodo `llm_call` via `skills_path` (un directorio contenedor con sub-skills) o `skills_paths` (lista de directorios).
- La única herramienta sintética expuesta es `load_skill({ name, reference? })`. Las `references` declaradas en el frontmatter de un `SKILL.md` son recursivas (límite de profundidad 5, con detección de ciclos).
- Documento completo: ver [`24_skills.md`](24_skills.md).

### Generación Multimedia
Nodos nativos para producir/editar imágenes y audio dentro del grafo: `image_generation`, `image_edit`, `tts`.
- Documento completo: ver [`32_multimedia_generation.md`](32_multimedia_generation.md).

### Biblioteca de Documentos (incluye HTML)
`ArtifactKind` soporta `Html` además de los formatos previos. Los assets se gestionan via los casos de uso `upload_asset` / `list_assets` / `delete_asset` sobre el puerto `AssetStore` (implementaciones `LocalFsAssetStore` y `GcsAssetStore`).
- Documento completo: ver [`27_documents_library.md`](27_documents_library.md).

---

## 10. Mantenimiento y Reset de Base de Datos

Para limpiar el estado de la aplicación y aplicar nuevos esquemas, se recomienda borrar las tablas en el orden correcto:

```sql
DROP TABLE IF EXISTS dag_runs CASCADE;
DROP TABLE IF EXISTS llm_node_history CASCADE;
DROP TABLE IF EXISTS dag_task_memory CASCADE;
DROP TABLE IF EXISTS dag_phase_summaries CASCADE;
DROP TABLE IF EXISTS _sqlx_migrations CASCADE;
```

Al reiniciar el motor con el CLI, se ejecutarán automáticamente las migraciones idempotentes definidas en `infrastructure/persistence/`.
