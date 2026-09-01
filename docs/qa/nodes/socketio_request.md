# QA — Nodo `socketio_request`

Fuente de código: `src/libs/colmena/src/dag_engine/infrastructure/nodes/socketio.rs`

Fuentes de doc revisadas:
- `docs/node_configurations.json` (entrada `socketio_request`, línea 1102)
- `docs/node_as_tools_reference.json` (entrada `socketio_request`, línea 432)
- `docs/agent_context/node_ports_reference.md` (tabla socketio_request)
- `docs/developer_guide/21_socketio_node.md` (guía completa)

---

## 1) Config documentada NO soportada por el código

**Sin discrepancias detectadas.**

Todos los campos documentados en `node_configurations.json` están implementados en el código:
- `url`, `namespace`, `event`, `payload`, `headers`, `cookies`, `wait_event`, `timeout_ms`, `transport`, `pre_events`
- Los defaults, tipos, y comportamientos (env var resolution, inputs > config) coinciden.
- Las validaciones fail-closed se aplican correctamente.

---

## 2) Código NO documentado

### 2.1 Error envelope: campos `exception` ausente en esquema schema()

**Hallazgo:** En `socketio.rs:102–105` (error_envelope en el schema), la doc lista:
```
"exception": "any (optional, present only for server-side exceptions caught via the 'exception' event)"
```

Sin embargo, el código **NO inyecta** un campo `exception` en el envelope de salida. El servidor puede emitir un evento `"exception"` (manejado en `socketio.rs:508–524`), que dispara el `exc_rx` channel y se agrega al error, pero el error en sí es una STRING en `{ ..., error: "<msg>" }`, no un campo `exception`.

**Código que evidencia esto:**
- Línea 289: `let msg = exc_val.get("message")...` — el servidor-exception se extrae COMO STRING.
- Línea 344: `Err(format!("server exception: {}", msg))` — se construye un error string.
- Línea 690: `"error": msg` — se inyecta solo como `error`, nunca como `exception`.

**Impacto para QA:** La doc de `node_configurations.json` promete un campo `exception` que el código no devuelve. Si un LLM depende de ese campo para inspeccionar el tipo de excepción del servidor, fallará silenciosamente.

---

### 2.2 Transport-error contexto (`transport_errors` + `advice`) ausente en schema() del nodo

**Hallazgo:** El código captura errores de transporte (línea 468: `transport_errors: Arc<Mutex<Vec<String>>>`) durante la ejecución y los inyecta en la envelope de salida (línea 695: `Self::attach_transport_context(&mut out, &transport_errors.lock().await)`). Sin embargo, el método `schema()` (línea 714–751) no documenta estos campos de salida.

**Código que evidencia esto:**
- Línea 471–489: handlers `on("error")` y `on("connect_error")` capturan errores (gated en `active`).
- Línea 176: `envelope["transport_errors"] = json!(Self::summarize_transport_errors(raw_errors))`.
- Línea 177: `envelope["advice"] = json!(TRANSPORT_ERROR_ADVICE)` (constante línea 55–58).
- Línea 748–749 en schema(): `"transport_errors": "array<string> (only on failure, when transport-level errors occurred...)"` — ESTÁ en el schema.

**Corrección:** El schema() SÍ documenta estos campos. Sin discrepancia aquí.

---

### 2.3 Validación de `transport` únicamente es "websocket|polling|any"; no hay rechazo de valores inválidos

**Hallazgo:** En `socketio.rs:394`, el valor `transport` se lee como string y se mapea en un `match` (línea 428–432):
```rust
let transport_type = match transport {
    "websocket" => TransportType::Websocket,
    "polling" => TransportType::Polling,
    _ => TransportType::Any,  // <-- cualquier valor invalido defaultea a Any
};
```

El código **no rechaza** valores inválidos de `transport` (p.ej., `"http"`, `"grpc"`). Defaultea silenciosamente a `Any`.

**Impacto para QA:** La doc dice `valid_values: ["websocket", "any", "polling"]`, pero el código no valida. Un grafo con `transport: "invalid_value"` se ejecutará sin error, usando `TransportType::Any` silenciosamente.

---

### 2.4 Env-var resolution en cabeceras: solo aplica a VALUES, no a KEYS

**Hallazgo:** En `socketio.rs:449–458`, el código itera sobre `headers` y solo resuelve env vars en los **valores** (`v_str`), no en las **claves** (`k`):
```rust
for (k, v) in headers {  // k = key never resolved
    if let Some(v_str) = v.as_str() {
        let v_resolved = Self::resolve_env_vars(v_str)?;  // solo v
        builder = builder.opening_header(k, v_resolved);  // k se pasa literal
    }
}
```

**Impacto para QA:** Si un operador intenta `"headers": { "${HEADER_NAME}": "value" }`, la clave no será resuelta. Esto es probablemente intencional (las claves de headers deben ser ASCII fijos), pero la doc no lo aclara explícitamente.

---

## 3) Plan de pruebas QA

Cubrir cada configuración distinta, defaults, casos límite, y errores fail-closed.

### Caso T1: Happy path — Ack mode simple (sin wait_event, sin pre_events)

**Objetivo:** Verificar que el nodo conecta a un servidor Socket.IO real, emite un evento, y recibe la respuesta via acknowledgment callback.

**Grafo mínimo:**
```json
{
  "metadata": { "requires_env": ["TEST_SOCKETIO_URL"] },
  "nodes": {
    "ping": {
      "type": "socketio_request",
      "config": {
        "url": "${TEST_SOCKETIO_URL}",
        "event": "ping",
        "payload": { "client_id": "test_qa_t1" },
        "timeout_ms": 5000
      }
    }
  },
  "edges": []
}
```

**Entrada:** ninguna (grafo sin `input` nodes; config self-contained).

**Resultado esperado:**
```json
{
  "success": true,
  "event": "ping",
  "response": { "pong": true, "server_time": "..." }
}
```

**Pass/Fail:** `success === true` Y `response` contiene datos del servidor.

---

### Caso T2: Wait-event mode — escuchar evento nombrado

**Objetivo:** Verificar que cuando `wait_event` está set, el nodo emite el evento y luego espera a una transmisión de servidor con ese nombre.

**Grafo mínimo:**
```json
{
  "metadata": { "requires_env": ["TEST_SOCKETIO_URL"] },
  "nodes": {
    "load_state": {
      "type": "socketio_request",
      "config": {
        "url": "${TEST_SOCKETIO_URL}",
        "namespace": "/test",
        "event": "request_state",
        "wait_event": "state_ready",
        "timeout_ms": 5000,
        "payload": { "request_id": "req_001" }
      }
    }
  },
  "edges": []
}
```

**Entrada:** ninguna.

**Resultado esperado:**
```json
{
  "success": true,
  "event": "request_state",
  "response": { "state": {...} }
}
```

**Verificación:** El servidor debe emitir `state_ready` en la misma conexión en respuesta a `request_state`. Sin `wait_event` defaultado, el nodo caería en ack mode y fallaría si el servidor no usa ack.

---

### Caso T3: Pre-events — secuencia ordenada en la misma conexión

**Objetivo:** Verificar que `pre_events` emite una secuencia de eventos en order sobre LA MISMA conexión, luego emite el evento principal.

**Grafo mínimo:**
```json
{
  "metadata": { "requires_env": ["TEST_SOCKETIO_URL"] },
  "nodes": {
    "create_with_room": {
      "type": "socketio_request",
      "config": {
        "url": "${TEST_SOCKETIO_URL}",
        "namespace": "/canvas",
        "event": "create_node",
        "timeout_ms": 10000,
        "pre_events": [
          {
            "event": "join_room",
            "payload": { "room_id": "room_abc" },
            "wait_event": "joined_room",
            "timeout_ms": 5000
          }
        ],
        "payload": {
          "room_id": "room_abc",
          "node_data": { "type": "text", "label": "Test Node" }
        }
      }
    }
  },
  "edges": []
}
```

**Entrada:** ninguna.

**Resultado esperado:**
```json
{
  "success": true,
  "event": "create_node",
  "response": { "node_id": "node_xyz", "created_at": "..." },
  "pre_responses": [
    { "event": "join_room", "response": { "success": true, "room_id": "room_abc" } }
  ]
}
```

**Verificación:**
- El array `pre_responses` está presente y contiene una entrada por cada pre_event que completó.
- El evento principal `create_node` se emitió DESPUÉS del pre_event `join_room`.

---

### Caso T4: Pre-event falla → abort, NO emitir main event

**Objetivo:** Verificar que si cualquier pre_event falla (timeout, server exception, channel error), el nodo NO emite el evento principal y devuelve `failed_pre_event`.

**Grafo mínimo:**
```json
{
  "metadata": { "requires_env": ["TEST_SOCKETIO_URL"] },
  "nodes": {
    "abort_on_pre_fail": {
      "type": "socketio_request",
      "config": {
        "url": "${TEST_SOCKETIO_URL}",
        "event": "create_node",
        "timeout_ms": 10000,
        "pre_events": [
          {
            "event": "invalid_setup",
            "payload": {},
            "timeout_ms": 2000
          }
        ],
        "payload": { "node_data": {...} }
      }
    }
  },
  "edges": []
}
```

**Entrada:** ninguna. El servidor NO responde a `invalid_setup` (timeout simulado).

**Resultado esperado:**
```json
{
  "success": false,
  "event": "create_node",
  "failed_pre_event": "invalid_setup",
  "error": "Timeout waiting for ack on 'invalid_setup' after 2000ms",
  "pre_responses": []
}
```

**Verificación:**
- `success === false`
- `failed_pre_event === "invalid_setup"`
- `pre_responses` está vacío (ningún pre_event completó antes del timeout)
- El evento principal `create_node` NO fue emitido (verificable contra logs del servidor)

---

### Caso T5: Timeout en main event → error envelope

**Objetivo:** Verificar que si el event principal timeout (ack o wait_event), el nodo devuelve error sin lanzar excepción.

**Grafo mínimo:**
```json
{
  "metadata": { "requires_env": ["TEST_SOCKETIO_URL"] },
  "nodes": {
    "timeout_main": {
      "type": "socketio_request",
      "config": {
        "url": "${TEST_SOCKETIO_URL}",
        "event": "slow_operation",
        "timeout_ms": 500,
        "payload": {}
      }
    }
  },
  "edges": []
}
```

**Entrada:** ninguna. El servidor es lento y no responde en < 500ms.

**Resultado esperado:**
```json
{
  "success": false,
  "event": "slow_operation",
  "error": "Timeout waiting for ack on 'slow_operation' after 500ms"
}
```

**Verificación:** `success === false`, `error` contiene "Timeout".

---

### Caso T6: Transport errors capturados → `transport_errors` + `advice`

**Objetivo:** Verificar que errores de transporte (EngineIO Error, connection drops) se capturan y se inyectan en el envelope con un consejo accionable para el LLM.

**Grafo mínimo:**
```json
{
  "metadata": { "requires_env": ["UNSTABLE_SOCKETIO_URL"] },
  "nodes": {
    "unreliable_connection": {
      "type": "socketio_request",
      "config": {
        "url": "${UNSTABLE_SOCKETIO_URL}",
        "event": "test",
        "timeout_ms": 5000
      }
    }
  },
  "edges": []
}
```

**Entrada:** ninguna. La URL apunta a un servidor o red inestable que dispara eventos `error` o `connect_error`.

**Resultado esperado:**
```json
{
  "success": false,
  "event": "test",
  "error": "failed to connect to ... : Connection refused",
  "transport_errors": ["EngineIO Error (x3)", "Connection reset (x1)"],
  "advice": "Transport-level errors occurred during this operation: the connection to the server is unstable or the server dropped the session. Retrying the same call is unlikely to help while these errors persist — if the problem continues, inform the user that the realtime backend appears to be unreachable."
}
```

**Verificación:**
- `transport_errors` es un array de strings (errores agregados)
- `advice` está presente alongside `transport_errors`
- Errores duplicados se colapsan con conteos (e.g., `"EngineIO Error (x3)"`)

---

### Caso T7: Env var resolution en payload (recursivo)

**Objetivo:** Verificar que strings dentro del payload se resuelven recursivamente para `${VAR_NAME}`.

**Grafo mínimo:**
```json
{
  "metadata": { "requires_env": ["TEST_SOCKETIO_URL", "TEST_ENV_ID", "TEST_LABEL"] },
  "nodes": {
    "env_resolved": {
      "type": "socketio_request",
      "config": {
        "url": "${TEST_SOCKETIO_URL}",
        "event": "create_item",
        "payload": {
          "environment_id": "${TEST_ENV_ID}",
          "nested": {
            "label": "${TEST_LABEL}",
            "config": { "path": "/data/${TEST_ENV_ID}/item" }
          }
        },
        "timeout_ms": 5000
      }
    }
  },
  "edges": []
}
```

**Entrada:** ninguna (config self-contained).

**Resultado esperado:**
- El payload enviado al servidor contiene:
  ```json
  {
    "environment_id": "env_prod_123",
    "nested": {
      "label": "Production Label",
      "config": { "path": "/data/env_prod_123/item" }
    }
  }
  ```

**Verificación:** Capturar el payload emitido en logs del servidor y verificar que todas las variables fueron resuelta.

---

### Caso T8: Transport default "websocket" (no polling)

**Objetivo:** Verificar que con `transport: "websocket"` (default), el nodo usa WebSocket unicast sin handshake de polling.

**Grafo mínimo:**
```json
{
  "metadata": { "requires_env": ["TEST_SOCKETIO_URL"] },
  "nodes": {
    "websocket_only": {
      "type": "socketio_request",
      "config": {
        "url": "${TEST_SOCKETIO_URL}",
        "event": "test",
        "transport": "websocket",
        "timeout_ms": 5000
      }
    }
  },
  "edges": []
}
```

**Entrada:** ninguna.

**Resultado esperado:** Conexión exitosa usando WebSocket (sin polling HTTP).

**Verificación:** (Verificable con packet sniffing o logs de librería rust_socketio) La primera conexión es un WebSocket upgrade directo, no una secuencia de polling.

---

### Caso T9: Transport "any" (polling-first + upgrade)

**Objetivo:** Verificar que con `transport: "any"`, el nodo intenta polling HTTP primero y luego actualiza a WebSocket.

**Grafo mínimo:**
```json
{
  "metadata": { "requires_env": ["TEST_SOCKETIO_URL"] },
  "nodes": {
    "polling_upgrade": {
      "type": "socketio_request",
      "config": {
        "url": "${TEST_SOCKETIO_URL}",
        "event": "test",
        "transport": "any",
        "timeout_ms": 5000
      }
    }
  },
  "edges": []
}
```

**Entrada:** ninguna.

**Resultado esperado:** Conexión exitosa. (Verificable en logs: primero HTTP long-polling, luego upgrade a WebSocket.)

---

### Caso T10: Transport "polling" (polling únicamente, sin upgrade)

**Objetivo:** Verificar que con `transport: "polling"`, el nodo usa solo HTTP long-polling, nunca actualiza a WebSocket.

**Grafo mínimo:**
```json
{
  "metadata": { "requires_env": ["TEST_SOCKETIO_URL"] },
  "nodes": {
    "polling_only": {
      "type": "socketio_request",
      "config": {
        "url": "${TEST_SOCKETIO_URL}",
        "event": "test",
        "transport": "polling",
        "timeout_ms": 5000
      }
    }
  },
  "edges": []
}
```

**Entrada:** ninguna.

**Resultado esperado:** Conexión exitosa usando HTTP long-polling.

**Verificación:** (Verificable en logs) Todas las solicitudes usan el protocolo `polling` sin intentar upgrade.

---

### Caso T11: Inputs override config

**Objetivo:** Verificar que input ports toman prioridad sobre config fields con el mismo nombre.

**Grafo mínimo:**
```json
{
  "nodes": {
    "input_override": {
      "type": "input",
      "config": {}
    },
    "socketio_call": {
      "type": "socketio_request",
      "config": {
        "url": "https://default.example.com",
        "event": "default_event",
        "payload": { "version": "from_config" }
      }
    }
  },
  "edges": [
    { "from": "input_override.url", "to": "socketio_call.url" },
    { "from": "input_override.event", "to": "socketio_call.event" },
    { "from": "input_override.payload", "to": "socketio_call.payload" }
  ]
}
```

**Entrada (via input_override):**
```json
{
  "url": "https://override.example.com",
  "event": "override_event",
  "payload": { "version": "from_input" }
}
```

**Resultado esperado:** El nodo usa `url`, `event`, `payload` del input, ignora config.

**Verificación:** Logs del servidor registran la solicitud con `override_event`, no `default_event`.

---

### Caso T12: Validation fail-closed: missing required `url`

**Objetivo:** Verificar que si `url` es missing y no viene de un input, el nodo falla con un error claro (sin conectar).

**Grafo mínimo:**
```json
{
  "nodes": {
    "no_url": {
      "type": "socketio_request",
      "config": {
        "event": "test"
      }
    }
  },
  "edges": []
}
```

**Entrada:** ninguna.

**Resultado esperado:**
```json
{
  "success": false,
  "error": "socketio_request: 'url' is required"
}
```
(Alternativamente, puede lanzar una excepción; la doc promise fail-closed envelope.)

**Verificación:** Mensaje de error contiene "url" y "required".

---

### Caso T13: Validation fail-closed: missing required `event`

**Objetivo:** Verificar que si `event` falta, el nodo falla con error claro.

**Grafo mínimo:**
```json
{
  "nodes": {
    "no_event": {
      "type": "socketio_request",
      "config": {
        "url": "https://api.example.com"
      }
    }
  },
  "edges": []
}
```

**Entrada:** ninguna.

**Resultado esperado:**
```json
{
  "success": false,
  "error": "socketio_request: 'event' is required"
}
```

**Verificación:** Mensaje de error contiene "event" y "required".

---

### Caso T14: Validation fail-closed: pre_events is not an array

**Objetivo:** Verificar que si `pre_events` no es un array (p.ej., objeto o string), falla en la validación.

**Grafo mínimo:**
```json
{
  "nodes": {
    "bad_pre_events": {
      "type": "socketio_request",
      "config": {
        "url": "https://api.example.com",
        "event": "test",
        "pre_events": { "event": "join" }
      }
    }
  },
  "edges": []
}
```

**Entrada:** ninguna.

**Resultado esperado:**
```json
{
  "success": false,
  "error": "socketio_request: 'pre_events' must be an array"
}
```

**Verificación:** Error message contiene "pre_events" y "array".

---

### Caso T15: Validation fail-closed: pre_events item missing `event` field

**Objetivo:** Verificar que cada item en `pre_events` valida que `event` es un string no-vacío.

**Grafo mínimo:**
```json
{
  "nodes": {
    "bad_pre_event_item": {
      "type": "socketio_request",
      "config": {
        "url": "https://api.example.com",
        "event": "main",
        "pre_events": [
          { "payload": {} }
        ]
      }
    }
  },
  "edges": []
}
```

**Entrada:** ninguna.

**Resultado esperado:**
```json
{
  "success": false,
  "error": "socketio_request: pre_events[0] requires non-empty 'event' string"
}
```

**Verificación:** Error message contiene index `[0]` y "event".

---

### Caso T16: Validation fail-closed: env var resolution failure

**Objetivo:** Verificar que si `${MISSING_VAR}` no existe en env, el nodo falla con error claro.

**Grafo mínimo:**
```json
{
  "nodes": {
    "missing_var": {
      "type": "socketio_request",
      "config": {
        "url": "${MISSING_SOCKETIO_VAR}",
        "event": "test"
      }
    }
  },
  "edges": []
}
```

**Entrada:** ninguna. La variable de env `MISSING_SOCKETIO_VAR` NO está set.

**Resultado esperado:**
```json
{
  "success": false,
  "error": "... Env var MISSING_SOCKETIO_VAR not found ..."
}
```

**Verificación:** Error message contiene el nombre de la variable faltante.

---

### Caso T17: Default namespace = "/"

**Objetivo:** Verificar que sin `namespace` en config, el nodo usa "/" (default).

**Grafo mínimo:**
```json
{
  "metadata": { "requires_env": ["TEST_SOCKETIO_URL"] },
  "nodes": {
    "default_namespace": {
      "type": "socketio_request",
      "config": {
        "url": "${TEST_SOCKETIO_URL}",
        "event": "test"
      }
    }
  },
  "edges": []
}
```

**Entrada:** ninguna.

**Resultado esperado:** Conexión exitosa a namespace "/".

**Verificación:** (En logs del servidor) La conexión es a namespace "/", no custom.

---

### Caso T18: Default timeout_ms = 10000 (10 segundos)

**Objetivo:** Verificar que sin `timeout_ms`, el nodo espera 10 segundos.

**Grafo mínimo:**
```json
{
  "metadata": { "requires_env": ["TEST_SOCKETIO_URL"] },
  "nodes": {
    "default_timeout": {
      "type": "socketio_request",
      "config": {
        "url": "${TEST_SOCKETIO_URL}",
        "event": "slow_test"
      }
    }
  },
  "edges": []
}
```

**Entrada:** ninguna. El servidor responde después de 8 segundos.

**Resultado esperado:** Exitoso (8s < 10s default).

**Verificación:** La solicitud completa sin timeout.

---

### Caso T19: Default payload = {} (empty object)

**Objetivo:** Verificar que sin `payload`, se envía `{}` al servidor.

**Grafo mínimo:**
```json
{
  "metadata": { "requires_env": ["TEST_SOCKETIO_URL"] },
  "nodes": {
    "no_payload": {
      "type": "socketio_request",
      "config": {
        "url": "${TEST_SOCKETIO_URL}",
        "event": "test"
      }
    }
  },
  "edges": []
}
```

**Entrada:** ninguna.

**Resultado esperado:** Payload recibido por el servidor es `{}`.

**Verificación:** Logs del servidor muestran event `test` con payload `{}`.

---

### Caso T20: Default pre_events = [] (empty array)

**Objetivo:** Verificar que sin `pre_events` o con `[]`, no hay pre-events ejecutados.

**Grafo mínimo:**
```json
{
  "metadata": { "requires_env": ["TEST_SOCKETIO_URL"] },
  "nodes": {
    "no_pre_events": {
      "type": "socketio_request",
      "config": {
        "url": "${TEST_SOCKETIO_URL}",
        "event": "test"
      }
    }
  },
  "edges": []
}
```

**Entrada:** ninguna.

**Resultado esperado:**
```json
{
  "success": true,
  "event": "test",
  "response": {...}
}
```

**Verificación:** No hay campo `pre_responses` en el output.

---

### Caso T21: Multiple pre_events con timeouts individuales

**Objetivo:** Verificar que cada pre_event puede tener su propio `timeout_ms`, fallback a node timeout si ausente.

**Grafo mínimo:**
```json
{
  "metadata": { "requires_env": ["TEST_SOCKETIO_URL"] },
  "nodes": {
    "multi_pre_timeouts": {
      "type": "socketio_request",
      "config": {
        "url": "${TEST_SOCKETIO_URL}",
        "event": "create",
        "timeout_ms": 10000,
        "pre_events": [
          {
            "event": "auth",
            "timeout_ms": 2000
          },
          {
            "event": "join_room",
            "timeout_ms": 5000
          }
        ],
        "payload": {}
      }
    }
  },
  "edges": []
}
```

**Entrada:** ninguna.

**Resultado esperado:**
- Pre-event `auth` espera max 2 segundos.
- Pre-event `join_room` espera max 5 segundos.
- Main event `create` espera max 10 segundos (fallback del node).

**Verificación:** Si `join_room` tarda 4s (< 5s timeout), continúa; si tarda 6s, timeout.

---

### Caso T22: Server exception via "exception" event

**Objetivo:** Verificar que el servidor puede emitir un evento `"exception"` que es capturado y reportado en el error envelope.

**Grafo mínimo:**
```json
{
  "metadata": { "requires_env": ["TEST_SOCKETIO_URL"] },
  "nodes": {
    "server_exception": {
      "type": "socketio_request",
      "config": {
        "url": "${TEST_SOCKETIO_URL}",
        "event": "invalid_operation",
        "timeout_ms": 5000
      }
    }
  },
  "edges": []
}
```

**Entrada:** ninguna. El servidor recibe `invalid_operation` y emite un evento `"exception"` en lugar de responder.

**Resultado esperado:**
```json
{
  "success": false,
  "event": "invalid_operation",
  "error": "server exception: Operation not allowed in current state"
}
```

**Verificación:** El error contiene "server exception" y el mensaje desde el servidor.

**Nota importante:** La doc de `node_configurations.json` promete un campo `exception` que NO está en el output (hallazgo 2.1). El error es STRING únicamente.

---

### Caso T23: Default output port = "response"

**Objetivo:** Verificar que edges sin especificar un output field reciben el valor de `response`.

**Grafo mínimo:**
```json
{
  "nodes": {
    "socketio_call": {
      "type": "socketio_request",
      "config": {
        "url": "https://api.example.com",
        "event": "get_data",
        "payload": {}
      }
    },
    "log_result": {
      "type": "log",
      "config": { "label": "Response" }
    }
  },
  "edges": [
    { "from": "socketio_call", "to": "log_result.message" }
  ]
}
```

**Entrada:** ninguna.

**Resultado esperado:** El nodo `log_result` recibe el valor del puerto `response` de `socketio_call` (no `success`, no `event`).

**Verificación:** Logs muestran solo el response data, no la envelope completa.

