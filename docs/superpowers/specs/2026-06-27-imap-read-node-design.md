# Nodo `imap_read` — lectura de correo IMAP (read-only)

- **Fecha:** 2026-06-27
- **Estado:** Diseño aprobado (brainstorming) — pendiente plan de implementación
- **Alcance v1:** IMAP genérico, read-only (search + fetch texto), listar adjuntos,
  descargar adjunto bajo demanda (como attachment de Colmena). Auth = app password
  (LOGIN plano sobre TLS). Enviar (SMTP) y XOAUTH2 quedan fuera (specs aparte / backlog).

## 1. Problema

Conectar un correo personal (Gmail u otro IMAP) a Colmena con **mínimo setup**, sin
GCP / OAuth / refresh tokens. Un **app password** (16 chars, requiere 2FA activo) sobre
IMAP resuelve el setup en minutos. Pero IMAP **no es HTTP**, así que el nodo
`http_request` (y su OAuth nativo) no sirve — se necesita un **nodo `imap_read` nuevo**.

Objetivo: un agente (`llm_call`) lee correos del buzón vía una tool, buscando por
criterios y recibiendo headers + cuerpo en texto, sin marcar leído y sin que el LLM
vea la contraseña.

## 2. Decisiones tomadas (brainstorming)

1. **Operaciones v1:** solo lectura — `SEARCH` + `FETCH` del contenido. No destructivo
   (EXAMINE read-only + `BODY.PEEK`). Sin marcar/mover/borrar.
2. **IMAP genérico:** `host`/`port`/`username`/`password` configurables, defaults a
   Gmail (`imap.gmail.com:993`, TLS implícito). Sirve para cualquier proveedor.
3. **Auth = app password** (LOGIN plano sobre TLS). XOAUTH2 fuera (más complejo, no
   menos; contradice "setup mínimo").
4. **Contenido devuelto:** headers (`from`/`to`/`subject`/`date`/`uid`) + cuerpo en
   **texto** (prefiere `text/plain`; si solo hay HTML, lo convierte a texto), truncado a
   `body_max_bytes`. Adjuntos: **siempre listados** (filename/mime/size); bytes
   descargados **bajo demanda** y registrados como attachment de Colmena.
5. **Búsqueda:** **criterios estructurados** (no query cruda). El nodo construye el
   comando `SEARCH`. Seguro para el LLM.
6. **Enviar (SMTP):** otro protocolo → **nodo `smtp_send` aparte** (spec futuro). Comparte
   el app password. Es acción hacia afuera/irreversible → requiere su propio cuidado.

## 3. Arquitectura / componentes

- Nodo nuevo `imap_read` (`ExecutableNode`), archivo
  `src/libs/colmena/src/dag_engine/infrastructure/nodes/imap.rs`.
- `imap_search.rs` — builder puro `criterios estructurados → comando IMAP SEARCH`
  (unit-testeable sin red).
- **Deps nuevas:**
  - `async-imap` — cliente IMAP async sobre tokio.
  - `tokio-rustls` — TLS (coincide con el stack rustls del proyecto).
  - `mail-parser` — parseo MIME puro-Rust (extrae texto + enumera adjuntos).
- Reusa `OutputStorageRepository` + attachment registry (inyectados con builders
  `with_storage` / `with_attachment_resolver`, igual que el nodo http) para registrar
  bytes de adjuntos descargados con un `document_id`.
- Registrado en `registry.rs` como `"imap_read"`.

## 4. Esquema de config

```json
{
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
```

- **Defaults:** `host=imap.gmail.com`, `port=993`, `mailbox=INBOX`, `max_results=20`,
  `body_max_bytes=5120`, `download_attachments=false`. `username`/`password` requeridos.
- Secretos (`password`) aceptan `${ENV}` o `secure_values`. Resolución `${ENV}` en nodo.
- `search` — todos los subcampos opcionales; ausencia = sin ese filtro. `search` vacío =
  todos los mensajes del mailbox (limitado por `max_results`).
- `download_attachments=true` → baja bytes de los adjuntos de los mensajes que coincidan
  (acotado por `max_results`) y los registra; cada adjunto del resultado incluye
  `document_id`.

## 5. Flujo de datos (no destructivo)

1. Conecta TLS a `host:port` (rustls). `LOGIN(username, password)`.
2. **`EXAMINE` `mailbox`** (modo read-only del protocolo → el servidor no muta flags) +
   `BODY.PEEK` en los fetch. Doble garantía de no marcar leído.
3. Construye `UID SEARCH <criterios>` (ver §6). Toma los últimos `max_results` UIDs.
4. Por cada UID: `UID FETCH BODY.PEEK[]` → `mail-parser`:
   - Extrae `from`, `to`, `subject`, `date`, `uid`.
   - Cuerpo: prefiere `text/plain`; si solo hay `text/html`, lo convierte a texto. Trunca
     a `body_max_bytes` (marca `body_truncated: true` si aplica).
   - Enumera adjuntos: `filename`, `mime`, `size`.
5. Si `download_attachments`: registra los bytes de cada adjunto en
   `OutputStorageRepository` / attachment registry → añade `document_id` a cada adjunto.
6. `LOGOUT`. Devuelve:

```json
{ "output": {
    "count": 2,
    "messages": [
      { "uid": 1234, "from": "...", "to": "...", "subject": "...", "date": "...",
        "body_text": "...", "body_truncated": false,
        "attachments": [ { "filename": "f.pdf", "mime": "application/pdf",
                           "size": 12345, "document_id": "doc_..." } ] }
    ] } }
```

## 6. Mapeo criterios → IMAP SEARCH (`imap_search.rs`, puro)

| Campo | SEARCH | Nota |
|-------|--------|------|
| `unseen: true` | `UNSEEN` | |
| `from: "x"` | `FROM "x"` | |
| `to: "x"` | `TO "x"` | |
| `subject: "x"` | `SUBJECT "x"` | |
| `body_contains: "x"` | `BODY "x"` | |
| `since: "2026-06-01"` | `SINCE 01-Jun-2026` | ISO → formato IMAP `dd-Mon-yyyy` |
| `before: "2026-06-27"` | `BEFORE 27-Jun-2026` | idem |

- Múltiples criterios se concatenan con AND implícito (IMAP `SEARCH A B C`).
- Sin criterios → `ALL`.
- `max_results` se aplica **después** del search (tomar los últimos N UIDs — más recientes).
- Validación: fechas mal formadas → error de config claro antes de conectar. Strings se
  escapan/citan correctamente para el protocolo (sin inyección de comandos).

## 7. Uso como tool de un LLM

Mismo patrón `node_schema+fixed` que el OAuth de http_request:
- `host`/`port`/`username`/`password`/`mailbox` → `fixed` (el LLM nunca los ve ni cambia).
- Solo `search` (y opcional `max_results`/`download_attachments`) son LLM-visibles.
- La contraseña jamás entra al schema, args ni resultado del tool.

**Nota de seguridad:** el contenido de correos es **entrada no confiable** (prompt-injection
contra el agente). Aquí no hay token que exfiltrar (no OAuth) y el nodo no hace requests a
hosts que el LLM controle. Pero cuando exista `smtp_send`, la combinación leer+enviar es un
vector — se aborda en ese spec (confirmación / anti-abuso).

## 8. Manejo de errores

- Fallo de conexión/TLS → error claro con host:port.
- `LOGIN` falla → mensaje accionable: *"autenticación IMAP fallida — verifica el app
  password y que el 2-Step Verification esté activo; en cuentas Workspace el admin puede
  tener IMAP/app-passwords deshabilitado"*.
- Mailbox inexistente → error con el nombre.
- `SEARCH` con 0 resultados → `{ count: 0, messages: [] }` (no es error).
- Parseo MIME de un mensaje falla → se omite ese mensaje con un `warning` en el resultado;
  no tumba el lote.
- `download_attachments=true` pero sin `OutputStorageRepository` inyectado → error claro
  (no silenciar): "imap_read: download_attachments requiere storage configurado".

## 9. Testing

- **Unit (sin red):**
  - `imap_search.rs`: cada criterio → fragmento SEARCH correcto; fechas ISO→IMAP;
    combinación múltiple; vacío → `ALL`; fecha inválida → error; escape de strings.
  - Wrapper `mail-parser`: fixtures RFC822 (texto plano, solo-HTML→texto, multipart con
    adjunto) → `body_text` + lista de adjuntos correctos; truncado respeta `body_max_bytes`.
- **E2E real (`#[ignore]`):** contra Gmail real con app password (creds desde env, nunca
  commit/print). No hay servidor IMAP embebido fácil para CI, así que el round-trip de
  protocolo se verifica E2E manual/gated. Grafo en `tests/graphs/external/`.

## 10. Compatibilidad / impacto ADP

- Aditivo: nodo nuevo registrado en `registry.rs`. **Deps nuevas** (`async-imap`,
  `tokio-rustls`, `mail-parser`) → suman tiempo de compilación y superficie de
  supply-chain (consciente). ADP worker lo toma en el próximo bump de colmena; sin
  breaking change. El canvas de ADP eventualmente necesitaría conocer el nodo (downstream,
  opcional, fuera de este spec).

## 11. Setup operativo (documentar)

- Activar **2-Step Verification** en la cuenta Google.
- Generar un **app password** (16 chars) en la config de seguridad de Google.
- Gmail **personal**: IMAP funciona con app password. Cuentas **Workspace** (p.ej.
  `@startti.co`): el admin puede tener IMAP/app-passwords deshabilitado — verificar antes.
- Usar `username` = dirección completa, `password` = el app password (como `secure_value`).

## 12. Fuera de alcance / backlog

- **`smtp_send`** (enviar correo) — spec/nodo aparte; comparte el app password.
- **XOAUTH2 sobre IMAP** — para cuentas sin app password / con OAuth obligatorio.
- **Mutaciones** (marcar leído, mover, borrar, etiquetar) — read-only en v1.
- **Fetch eficiente vía `BODYSTRUCTURE`** — v1 baja el mensaje completo (`BODY.PEEK[]`)
  aunque `download_attachments=false`; la optimización de bajar solo la parte de texto (y
  adjuntos solo si se piden) es Fase 2.
- **Descarga de adjunto por UID+parte específica** (en vez del flag que baja todos) —
  refinamiento futuro si se necesita control fino.
