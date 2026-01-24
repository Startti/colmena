# 🔌 EPIC 3: External Integrations

**Objetivo**: Conectar WhatsApp y Slack mediante Webhooks (Inbound) y Nodos (Outbound).

---

## 📋 Lista de Tareas

### INT-01: Webhook Ingress API
**Objetivo**: Punto de entrada único para eventos externos.

1.  **Endpoint Genérico (`src/platform/api/src/webhooks.rs`)**:
    *   `POST /webhooks/:source/:dag_id`.
    *   `source`: `whatsapp`, `slack`, etc.
    *   `dag_id`: ID del DAG que debe ejecutarse al recibir este evento.

2.  **Factory Logic**:
    *   `match source`:
        *   `"whatsapp"` -> `normalize_whatsapp(body, headers)`.
        *   `"slack"` -> `normalize_slack(body, headers)`.
        *   `_` -> `404 Not Found`.

3.  **Enqueue**:
    *   El resultado de `normalize` son los `inputs` para el DAG.
    *   Buscar el DAG en DB (o usar uno predefinido).
    *   Construir `JobRequest` y hacer `LPUSH`.

### INT-02: WhatsApp Adapter
**Objetivo**: Entender y validar los mensajes de Meta.

1.  **HMAC Validation**:
    *   Leer header `X-Hub-Signature-256`.
    *   Calcular HMAC-SHA256 del body con `APP_SECRET`.
    *   Comparar (timing-safe). Si falla, `401 Unauthorized`.

2.  **Normalization Logic**:
    *   Extraer mensaje de texto: `entry[0].changes[0].value.messages[0].text.body`.
    *   Extraer sender: `entry[0].changes[0].value.messages[0].from`.
    *   Retornar `serde_json::json!({ "message": text, "sender": phone })`.

### INT-03: WhatsApp Send Node
**Objetivo**: Enviar respuestas a usuarios.

1.  **Estructura del Nodo (`src/libs/dag_engine/infrastructure/nodes/whatsapp.rs`)**:
    *   Type: `whatsapp_send`.
    *   Inputs: `to` (teléfono), `body` (texto).
    *   Config: `api_token` (o tomar de ENV), `phone_number_id`.

2.  **Ejecución**:
    *   Usar `reqwest` para POST a `https://graph.facebook.com/v17.0/{phone_number_id}/messages`.
    *   Auth: Bearer Token.
    *   Body: Standard WhatsApp JSON format.
    *   Retorno: `{ "status": "sent", "message_id": "..." }`.

### INT-04: Slack Integration (Opcional)
**Objetivo**: Similar a WhatsApp pero para Slack.

1.  **Slack Event API**:
    *   Manejar `url_verification` challenge.
2.  **Slack Post Node**:
    *   Type: `slack_post`.
    *   Config: `webhook_url` o `bot_token` + `channel_id`.
