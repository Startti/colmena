# Vercel AI SDK Data Stream Protocol Specification

Documentación técnica para la implementación correcta del protocolo Data Stream (v1) en entornos backend de Rust (e.g., Axum). 

Este protocolo permite streaming en tiempo real de texto, llamadas a herramientas y metadatos de ejecución, siendo compatible nativamente con componentes de UI del AI SDK como `useChat`.

## Requisitos de Cabecera (Headers)

Para que el cliente (AI SDK) reconozca automáticamente el stream de datos estructurados, la respuesta HTTP debe incluir:

```http
Content-Type: text/event-stream
Cache-Control: no-cache
Connection: keep-alive
x-vercel-ai-ui-message-stream: v1
```

## Formato del Stream (Server-Sent Events)

Cada mensaje es un evento SSE con el prefijo `data: ` seguido de un objeto JSON y terminado por `\n\n`.

### Tipos de Partes del Mensaje

#### 1. Start (Inicio)
Primer mensaje del stream.

```json
{
  "type": "start",
  "messageId": "msg_uuid_v4"
}
```

#### 2. Bloques de Texto
El texto se envía en bloques delimitados por un ID único.

*   **Inicio de Texto (`text-start`):**
    ```json
    {
      "type": "text-start",
      "id": "txt_uuid_v4" // Debe ser un UUID único para este bloque
    }
    ```

*   **Delta de Texto (`text-delta`):**
    ```json
    {
      "type": "text-delta",
      "id": "txt_uuid_v4", // Debe coincidir con el ID de text-start
      "delta": " fragmento de texto"
    }
    ```

*   **Fin de Texto (`text-end`):**
    ```json
    {
      "type": "text-end",
      "id": "txt_uuid_v4"
    }
    ```

#### 3. Llamadas a Herramientas (Tool Calls)
Los argumentos de herramientas se envían progresivamente.

*   **Inicio de Entrada (`tool-input-start`):** (Opcional si se usa delta directamente)
*   **Delta de Entrada (`tool-input-delta`):**
    > **IMPORTANTE**: El campo `toolCallId` es obligatorio en **todos** los chunks, no solo en el primero.
    ```json
    {
      "type": "tool-input-delta",
      "toolCallId": "call_abc123",
      "inputTextDelta": "{\"arg\": \"val\"}" // Fragmento parcial del JSON de argumentos
    }
    ```
*   **Entrada Completa Disponible (`tool-input-available`):** (Opcional, útil para depuración)
*   **Salida de Herramienta (`tool-output-available`):**
    ```json
    {
      "type": "tool-output-available",
      "toolCallId": "call_abc123",
      "output": { "result": 42 } // Objeto JSON con el resultado
    }
    ```

#### 4. Control de Ejecución (Finish)

*   **Finish Step (`finish-step`):**
    Se envía al finalizar cada llamada individual al LLM (ej. antes de ejecutar una herramienta).
    ```json
    {
      "type": "finish-step",
      "finishReason": "stop", // o "tool_calls"
      "usage": {
        "promptTokens": 100,
        "completionTokens": 20
      }
    }
    ```

*   **Finish Final (`finish`):**
    Se envía **una sola vez** al terminar toda la ejecución del grafo. Debe contener el uso acumulado.
    ```json
    {
      "type": "finish",
      "finishReason": "stop",
      "usage": {
        "promptTokens": 500, // Total acumulado
        "completionTokens": 100
      }
    }
    ```

#### 5. Terminación Protocolar
El stream debe terminar estrictamente con el string literal:

```text
data: [DONE]

```

---

## Ejemplo Completo del Flujo (Dump de `curl`)

```text
data: {"messageId":"msg_c55a3...","type":"start"}

data: {"inputTextDelta":"","toolCallId":"call_abc...","type":"tool-input-delta"}
data: {"inputTextDelta":"{\"a\":1}","toolCallId":"call_abc...","type":"tool-input-delta"}

data: {"id":"txt_aa07c...","type":"text-start"}
data: {"delta":"Calculating...","id":"txt_aa07c...","type":"text-delta"}

data: {"finishReason":"stop","type":"finish-step","usage":{"completionTokens":17,"promptTokens":145}}

data: {"input":{"a":1},"toolCallId":"call_abc...","toolName":"add","type":"tool-input-available"}
data: {"output":{"result":42},"toolCallId":"call_abc...","type":"tool-output-available"}

data: {"id":"txt_aa07c...","type":"text-end"}

data: {"finishReason":"stop","type":"finish","usage":{"completionTokens":55,"promptTokens":528}}

data: [DONE]
```

---

## Integración Frontend (Next.js / React)

Ejemplo de consumo usando `useChat` de `ai/react`.

```tsx
'use client';
import { useChat } from 'ai/react';

export default function Chat() {
  const { messages, input, handleInputChange, handleSubmit } = useChat({
    api: 'http://localhost:3000/agent-demo-stream',
  });

  return (
    <div>
      {messages.map(m => (
        <div key={m.id}>
          <strong>{m.role}:</strong> {m.content}
          
          {/* Renderizado de herramientas */}
          {m.toolInvocations?.map(tool => (
            <div key={tool.toolCallId} className="bg-gray-100 p-2 my-2 rounded">
              Running tool: {tool.toolName}...
              {'result' in tool && <div>Result: {JSON.stringify(tool.result)}</div>}
            </div>
          ))}
        </div>
      ))}
      
      <form onSubmit={handleSubmit}>
        <input value={input} onChange={handleInputChange} />
        <button type="submit">Send</button>
      </form>
    </div>
  );
}
```
