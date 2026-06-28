# Nodo IMAP (`imap_read`)

Nodo de **lectura de correo por IMAP**, read-only. Conecta un buzón personal o de
empresa (Gmail u otro proveedor IMAP) a un agente de Colmena con **mínimo setup**: un
**app password** sobre TLS, sin GCP / OAuth / refresh tokens. IMAP no es HTTP, así que el
nodo `http_request` no sirve — `imap_read` es el nodo dedicado.

Caso de uso típico: un `llm_call` lee correos del buzón vía una tool, buscando por
criterios estructurados y recibiendo headers + cuerpo en texto, **sin marcar como leído**
y **sin que el LLM vea la contraseña**.

> Spec de diseño: [`docs/superpowers/specs/2026-06-27-imap-read-node-design.md`](../superpowers/specs/2026-06-27-imap-read-node-design.md).

## Qué hace

- Conecta por TLS a `host:port` (rustls), hace `LOGIN(username, password)`.
- Abre el `mailbox` en modo **read-only** del protocolo (`EXAMINE`) y usa `BODY.PEEK` en
  los fetch → doble garantía de que el servidor **no muta flags** (no marca leído).
- Construye un comando `UID SEARCH` a partir de criterios estructurados (ver §
  [Criterios de búsqueda](#criterios-de-búsqueda)), toma los últimos `max_results` UIDs
  (los más recientes).
- Por cada mensaje: parsea MIME (crate `mail-parser`), extrae headers + cuerpo en texto,
  enumera adjuntos. Opcionalmente descarga los bytes de los adjuntos.
- `LOGOUT`. Devuelve el lote de mensajes.

## Esquema de configuración

```json
{
  "type": "imap_read",
  "config": {
    "host": "imap.gmail.com",
    "port": 993,
    "username": "${GMAIL_USER}",
    "password": "${GMAIL_APP_PASSWORD}",
    "mailbox": "INBOX",
    "search": {
      "unseen": true,
      "from": "jefe@x.com",
      "to": "yo@x.com",
      "subject": "factura",
      "body_contains": "urgente",
      "since": "2026-06-01",
      "before": "2026-06-27"
    },
    "max_results": 20,
    "body_max_bytes": 5120,
    "download_attachments": false
  }
}
```

| Campo | Tipo | Default | Descripción |
|-------|------|---------|-------------|
| `host` | string | `imap.gmail.com` | Host del servidor IMAP. |
| `port` | integer | `993` | Puerto (TLS implícito). |
| `username` | string | — (**requerido**) | Dirección completa de correo. |
| `password` | string | — (**requerido**) | App password. Usa `${ENV}` o `secure_value` — **nunca** literal. |
| `mailbox` | string | `INBOX` | Buzón a abrir (EXAMINE). |
| `search` | object | `{}` | Criterios estructurados (todos opcionales; ver abajo). |
| `max_results` | integer | `20` | Máximo de mensajes a traer (los más recientes). |
| `body_max_bytes` | integer | `5120` | Trunca `body_text` a este tamaño. |
| `download_attachments` | boolean | `false` | Si `true`, descarga y registra los bytes de cada adjunto. |

- El `password` acepta `${ENV}` o `secure_values`; la resolución de `${ENV}` ocurre en el
  nodo. **Nunca** pongas la contraseña en claro en el grafo.
- `search` vacío (o ausente) = todos los mensajes del mailbox, acotados por `max_results`.

## Criterios de búsqueda

`imap_read` **no** acepta una query IMAP cruda — usa criterios estructurados que el nodo
traduce a un comando `SEARCH` seguro (sin inyección de comandos; strings citados/escapados,
fechas validadas antes de conectar). Mapeo (`imap_search.rs`, puro y unit-testeable):

| Campo | SEARCH | Nota |
|-------|--------|------|
| `unseen: true` | `UNSEEN` | Solo no leídos. |
| `from: "x"` | `FROM "x"` | |
| `to: "x"` | `TO "x"` | |
| `subject: "x"` | `SUBJECT "x"` | |
| `body_contains: "x"` | `BODY "x"` | |
| `since: "2026-06-01"` | `SINCE 01-Jun-2026` | ISO → formato IMAP `dd-Mon-yyyy`. |
| `before: "2026-06-27"` | `BEFORE 27-Jun-2026` | idem. |

- Múltiples criterios se concatenan con **AND implícito** (IMAP `SEARCH A B C`).
- Sin criterios → `ALL`.
- `max_results` se aplica **después** del search (toma los últimos N UIDs, los más
  recientes).
- Una fecha mal formada produce un **error de config claro antes de conectar**.

## Comportamiento read-only

El nodo es **no destructivo** por diseño:

- Abre el buzón con `EXAMINE` (modo read-only del protocolo: el servidor no muta flags).
- Hace los fetch con `BODY.PEEK[]` (a diferencia de `BODY[]`, que marcaría `\Seen`).

La combinación de ambos garantiza que **leer un correo con `imap_read` no lo marca como
leído**. v1 no soporta mutaciones (marcar, mover, borrar, etiquetar) — ver
[Backlog](#fuera-de-alcance--backlog).

## Adjuntos

Los adjuntos se **listan siempre** (filename / mime / size), independientemente de
`download_attachments`. Los bytes se descargan **bajo demanda**:

- `download_attachments: false` (default): cada adjunto del resultado trae solo metadata.
- `download_attachments: true`: el nodo baja los bytes de cada adjunto de los mensajes que
  coincidan (acotado por `max_results`), los registra en `OutputStorageRepository` /
  attachment registry, y añade un `document_id` a cada adjunto. Ese `document_id` puede
  luego forwardearse a otros nodos (p.ej. `$attachment:<document_id>` en `http_request`
  multipart).

> Si `download_attachments: true` pero el nodo no recibió un `OutputStorageRepository`
> inyectado (builders `with_storage` / `with_attachment_resolver`), falla con un error
> claro — no se silencia.

## Salida

```json
{ "output": {
    "count": 2,
    "messages": [
      { "uid": 1234,
        "from": "...", "to": "...", "subject": "...", "date": "...",
        "body_text": "...", "body_truncated": false,
        "attachments": [
          { "filename": "factura.pdf", "mime": "application/pdf",
            "size": 12345, "document_id": "doc_..." }
        ] }
    ] } }
```

- `body_text` prefiere `text/plain`; si solo hay `text/html`, se convierte a texto. Si se
  truncó a `body_max_bytes`, `body_truncated` es `true`.
- `document_id` aparece en un adjunto solo cuando `download_attachments: true`.

## Uso como tool de un LLM

Mismo patrón `node_schema+fixed` que el OAuth nativo de `http_request`: los campos de
conexión van **fijos** en `node_schema`, de modo que el LLM no los ve ni los puede cambiar
— **la contraseña jamás entra al schema, a los args, ni al resultado del tool**.

```json
"tool_configurations": {
  "read_email": {
    "name": "read_email",
    "node_type": "imap_read",
    "description": "Lee correos del buzón del usuario (read-only). Usa search para filtrar.",
    "node_schema": {
      "host":     { "type": "string",  "fixed": "imap.gmail.com" },
      "port":     { "type": "integer", "fixed": 993 },
      "username": { "type": "string",  "fixed": "${GMAIL_USER}" },
      "password": { "type": "string",  "fixed": "${GMAIL_APP_PASSWORD}" },
      "mailbox":  { "type": "string",  "fixed": "INBOX" },
      "max_results": { "type": "integer", "fixed": 5 },
      "search": {
        "type": "object",
        "required": false,
        "description": "Criterios opcionales: unseen, from, to, subject, body_contains, since, before.",
        "properties": {
          "unseen":        { "type": "boolean", "required": false },
          "from":          { "type": "string",  "required": false },
          "subject":       { "type": "string",  "required": false },
          "body_contains": { "type": "string",  "required": false }
        }
      }
    }
  }
}
```

Solo `search` (y opcionalmente `max_results` / `download_attachments`) deberían ser
LLM-visibles. `host`/`port`/`username`/`password`/`mailbox` siempre `fixed`.

> **Nota de seguridad.** El contenido de un correo es **entrada no confiable**
> (prompt-injection contra el agente). Aquí no hay token que exfiltrar (no es OAuth) y el
> nodo no hace requests a hosts que el LLM controle. Pero cuando exista `smtp_send`, la
> combinación leer + enviar es un vector — se abordará en ese spec.

## Manejo de errores

- **Conexión/TLS** falla → error claro con `host:port`.
- **`LOGIN` falla** → mensaje accionable: *"autenticación IMAP fallida — verifica el app
  password y que el 2-Step Verification esté activo; en cuentas Workspace el admin puede
  tener IMAP / app-passwords deshabilitado"*.
- **Mailbox inexistente** → error con el nombre.
- **`SEARCH` con 0 resultados** → `{ count: 0, messages: [] }` (no es error).
- **Parseo MIME de un mensaje falla** → ese mensaje se **omite** con un `warning` en el
  resultado; no tumba el lote completo.

## Setup operativo (app password)

1. Activa **2-Step Verification (2FA)** en la cuenta de Google.
2. Genera un **app password** (16 caracteres) en la configuración de seguridad de Google.
   Se genera **una sola vez** — guárdalo en `secure_values` o como env var.
3. **Gmail personal:** IMAP funciona con app password.
   **Cuentas Workspace** (p.ej. `@startti.co`): el admin puede tener IMAP / app-passwords
   **deshabilitado** — verifícalo antes de configurar el nodo.
4. Usa `username` = dirección completa, `password` = el app password (como `secure_value`
   o `${ENV}`).

## Fuera de alcance / backlog

- **`smtp_send` (enviar correo)** — es otro protocolo → será un **nodo aparte** (spec
  futuro). Compartirá el app password. Al ser una acción hacia afuera / irreversible,
  requiere su propio cuidado (confirmación / anti-abuso).
- **XOAUTH2 sobre IMAP** — para cuentas sin app password / con OAuth obligatorio.
- **Mutaciones** (marcar leído, mover, borrar, etiquetar) — read-only en v1.
- **Fetch eficiente vía `BODYSTRUCTURE`** — v1 baja el mensaje completo aunque
  `download_attachments=false`; la optimización es Fase 2.
