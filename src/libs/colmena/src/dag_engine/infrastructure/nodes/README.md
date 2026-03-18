# Nodos de Colmena

Resumen de los nodos disponibles en este directorio (`src/libs/colmena/src/dag_engine/infrastructure/nodes`), agrupados por funcionalidad.

## 🛠️ Depuración (`debug.rs`)
*   **`LogNode`**: Imprime sus entradas en la consola del servidor. Útil para inspeccionar valores intermedios durante la ejecución del grafo.
*   **`MockInputNode`**: Un nodo especial diseñado para devolver su propia configuración como output. Se usa principalmente para simular entradas en pruebas.

## 🌐 HTTP (`http.rs`)
*   **`HttpNode`**: Permite realizar peticiones HTTP (GET, POST, PUT, DELETE) a servicios externos.
    *   Soporta configuración dinámica de URL, headers y body mediante inputs.
    *   Incluye resolución de variables de entorno (sintaxis `${VAR}`).

## 🧠 Inteligencia Artificial (`llm.rs`)
*   **`LlmNode`**: Nodo para invocar Modelos de Lenguaje (LLMs).
    *   **Proveedores**: Soporta OpenAI, Gemini y Anthropic.
    *   **Capacidades**:
        *   Manejo de historial de conversación (memoria) si se provee un `thread_id`.
        *   Ejecución de herramientas (Function Calling).
        *   Streaming de tokens (opcional).
        *   Configuración de `system_message`, temperatura, max_tokens, etc.

## 📝 Extracción de Información (`extraction.rs`)
*   **`ExtractionNode`**: Nodo para extraer información estructurada a partir de texto no estructurado.
    *   **Identificador**: `information_extraction`.
    *   **Funcionamiento**: Recibe un mapa de textos (`texts`) y un esquema JSON (`schema`). Utiliza un modelo de lenguaje para procesar los textos y generar una salida validada en formato JSON que coincide estrictamente con el esquema solicitado.
    *   **Soporte Multi-Entrada**: Permite inyectar múltiples fragmentos de texto o documentos al mismo nodo definiendo variables en `texts` a través de los *edges* del DAG (ej. `node_foo.texts.doc1`, `node_foo.texts.doc2`).

## 🧮 Matemáticas (`math.rs`)
Operaciones aritméticas básicas para manipular valores numéricos:
*   **`AddNode`**: Suma (`a + b`).
*   **`SubtractNode`**: Resta (`a - b`).
*   **`MultiplyNode`**: Multiplica (`a * b`).
*   **`DivideNode`**: Divide (`a / b`). Maneja error de división por cero.
*   **`ExponentialNode`**: Potenciación, donde la base viene del input y el exponente de la configuración.

## 🐍 Ejecución de Código (`python_node.rs`)
*   **`PythonNode`**: Ejecuta scripts de Python arbitrarios de forma aislada.
    *   Inyecta los `inputs` del nodo como variables globales en el script.
    *   Captura el valor de la variable `output` definida en el script como resultado del nodo.
    *   Soporta importación de librerías estándar.

## ⚡ Disparadores (`trigger.rs`)
*   **`TriggerWebhookNode`**: Actúa como punto de entrada para flujos iniciados externamente (webhooks).
    *   Toma el payload recibido (o un payload de prueba configurado) y lo pasa como output para que otros nodos lo consuman.
