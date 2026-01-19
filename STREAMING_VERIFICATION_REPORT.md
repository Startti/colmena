# Reporte de Verificación: Streaming Extendido de LLM

Este documento detalla los resultados de la prueba de verificación `tests/verify_tool_stream.py` y explica el significado de cada evento capturado durante la ejecución del flujo de streaming en Colmena.

## Resumen de la Ejecución
La prueba verificó la capacidad del sistema para:
1.  Interceptar una solicitud con `stream=true`.
2.  Transmitir **llamadas a herramientas (Tool Calls)** de manera incremental a través de SSE.
3.  Transmitir **métricas de uso (Usage)** (tokens de prompt y completado).
4.  Reconstruir y ejecutar la herramienta correctamente en el backend.
5.  Transmitir la respuesta final generada por el LLM.

La prueba fue **EXITOSA**.

## Detalle de Eventos Capturados

A continuación, se explican los eventos clave observados en la salida de la prueba (logs reales):

### 1. `llm_tool_call` (Streaming de Herramientas)
```text
Event: message, Data: {"type":"data","value":{"data":{"args_chunk":"","node_id":"llm","tool_id":"call_lompBHq0hPhoFvuAV71BpQ7K","tool_name":"mock_weather"},"event":"llm_tool_call"}}
Event: message, Data: {"type":"data","value":{"data":{"args_chunk":"{\"","node_id":"llm","tool_id":"","tool_name":""},"event":"llm_tool_call"}}
Event: message, Data: {"type":"data","value":{"data":{"args_chunk":"query","node_id":"llm","tool_id":"","tool_name":""},"event":"llm_tool_call"}}
Event: message, Data: {"type":"data","value":{"data":{"args_chunk":"_params","node_id":"llm","tool_id":"","tool_name":""},"event":"llm_tool_call"}}
...
```

### 2. `llm_usage` (Métricas de Uso)
```text
Event: message, Data: {"type":"data","value":{"data":{"completion_tokens":17,"node_id":"llm","prompt_tokens":76},"event":"llm_usage"}}
```

### 3. `llm_token` (Contenido de Texto)
```text
Event: message, Data: {"type":"data","value":{"data":{"node_id":"llm","token":"The"},"event":"llm_token"}}
Event: message, Data: {"type":"data","value":{"data":{"node_id":"llm","token":" weather"},"event":"llm_token"}}
Event: message, Data: {"type":"data","value":{"data":{"node_id":"llm","token":" in"},"event":"llm_token"}}
...
```

### 4. `graph_finish` (Fin de Ejecución)
```text
Event: message, Data: {"type":"data","value":{"data":{"output":{"output":{"content":"The weather in Paris is currently sunny with a temperature of 25°C.","tool_calls":null,"usage":{"completion_tokens":16,"prompt_tokens":154,"total_tokens":170}}}},"event":"graph_finish"}}
```

## Conclusión
La implementación de `LlmStreamPart` y las modificaciones en los adaptadores permiten ahora una visibilidad completa y granular del proceso de generación del LLM, habilitando interfaces de usuario más ricas y reactivas que pueden mostrar herramientas en ejecución y consumo de recursos en tiempo real.

## Instrucciones de Ejecución

Para replicar esta prueba, siga los siguientes pasos:

### 1. Iniciar el Servidor Colmena
Ejecute el siguiente comando en la raíz del proyecto para iniciar el servidor con la configuración de prueba:

```bash
set -a && source .env && set +a && cargo run --bin dag_engine -- serve tests/dags/llm_stream_tool.json
```

Esto iniciará el servidor en `http://localhost:3000`.

**Nota sobre `set -a` y `set +a`:**
Estos comandos se utilizan para cargar las variables del archivo `.env` en el entorno actual para que `cargo run` pueda acceder a ellas (como `OPENAI_API_KEY`).
*   `set -a`: Activa la exportación automática de variables.
*   `source .env`: Carga las variables del archivo `.env`.
*   `set +a`: Desactiva la exportación automática.

### Configuración personalizada de Host y Puerto

Si desea cambiar el host o el puerto, puede usar los argumentos `--host` y `--port`:

```bash
set -a && source .env && set +a && cargo run --bin dag_engine -- serve tests/dags/llm_stream_tool.json --host 127.0.0.1 --port 8080
```

Recuerde actualizar también la URL en el script de Python (`COLMENA_URL`) si cambia el puerto.

### 2. Ejecutar el Script de Verificación
En otra terminal, ejecute el script de Python (asegúrese de usar el entorno virtual):

```bash
.venv/bin/python tests/verify_tool_stream.py
```

### Código de Verificación (`tests/verify_tool_stream.py`)

A continuación se muestra el código completo utilizado para la verificación:

```python
import requests
import json
import sseclient
import threading
from http.server import BaseHTTPRequestHandler, HTTPServer
import time
import os

# 1. Mock Weather API Server
class WeatherHandler(BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.send_header('Content-type', 'application/json')
        self.end_headers()
        self.wfile.write(json.dumps({"weather": "Sunny", "temp": 25}).encode())
    
    def log_message(self, format, *args):
        pass

def run_weather_server():
    server = HTTPServer(('localhost', 8085), WeatherHandler)
    print("🌤️ Weather server running on 8085...")
    server.serve_forever()

threading.Thread(target=run_weather_server, daemon=True).start()

# 2. Execute DAG and Stream
COLMENA_URL = "http://localhost:3000/execute"

# Payload is just the prompt now
payload = {"prompt": "What is the weather in Paris?"}

print(f"\n🚀 Sending request to {COLMENA_URL}...")
response = requests.post(COLMENA_URL, json=payload, stream=True, headers={'Accept': 'text/event-stream'})

client = sseclient.SSEClient(response)

tool_call_seen = False
usage_seen = False
content_received = ""

print("\n📡 Listening for SSE events...")
for event in client.events():
    print(f"Event: {event.event}, Data: {event.data}")
    
    try:
        payload = json.loads(event.data)
        if "value" in payload and "event" in payload["value"]:
            event_type = payload["value"]["event"]
            data = payload["value"].get("data", {})
            
            if event_type == "llm_tool_call":
                tool_call_seen = True
                print(f"🔧 Tool Call Chunk: {data.get('tool_name', '')} args={data.get('args_chunk', '')}")
            
            elif event_type == "llm_usage":
                usage_seen = True
                print(f"📊 Usage: {data}")

            elif event_type == "llm_token":
                token = data.get('token', '')
                content_received += token
                print(f"📝 Token: {token}", end="", flush=True)

            elif event_type == "graph_finish":
                print("\n🏁 Graph Finished")
                break
    except Exception as e:
        print(f"Error parsing event: {e}")

print(f"\n\n✅ Verification Results:")
print(f"Tool Call Seen: {tool_call_seen}")
print(f"Usage Seen: {usage_seen}")

if tool_call_seen and usage_seen:
    print("🎉 SUCCESS: Streamed tool calls and usage!")
    exit(0)
else:
    print("❌ FAILURE: Missing expected events.")
    exit(1)
```
