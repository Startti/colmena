# 45. Google Docs integration (Subsystem G)

> v1 ships 22 synthetic LLM tools que reflejan el modelo de edición
> quirúrgica direccionada por contenido — el agente describe **qué**
> cambiar (texto, encabezado, named range), nunca offsets UTF-16. Soporte
> multi-tab, conversión markdown ↔ Docs con detección de pérdidas, y
> seguridad ante co-edición concurrente vía revisionId equality check +
> tabla postgres `gdocs_session_state`. Auth vía Service Account JSON o
> Application Default Credentials (sin OAuth user-scoped en v1).
>
> **Live-verified 2026-06-09** contra un Google Doc real compartido por
> el usuario (`1QkeEG4PU0PFBwDs8dP6WaYUIEafVwjL3D27eA1w8f0k`, SA
> `colmena-sheets-tester@startti-dev.iam.gserviceaccount.com`). El flujo
> completo "agente arma plan en tab nuevo → usuario edita a mano → agente
> detecta `human_changes_pending` → llama `acknowledge_human_changes` →
> retry exitoso" funciona end-to-end. Ver §"Live verification findings"
> al final de [la spec](../superpowers/specs/2026-06-08-google-docs-design.md).

## Por qué este diseño

Google Docs no expone un modelo "celda" como Sheets. Internamente el doc
es un stream lineal de tokens (paragraphs, runs, elements) indexados por
posición UTF-16. Pedirle al LLM que calcule esos índices es una receta
para off-by-one silentes. v1 elimina por completo esa superficie de
error con tres decisiones:

1. **Content-addressed.** El agente dice "reemplaza `find` por `replace`
   en `scope`". El servidor resuelve los índices con `documents.get` →
   parser de outline + búsqueda textual. Si hay ambigüedad, devuelve un
   error tipado en lugar de adivinar.
2. **Markdown como I/O.** Toda creación e inserción de contenido se
   recibe como markdown y se traduce a requests del API mediante el
   converter en
   `dag_engine/infrastructure/nodes/llm_synthetic_tools/markdown_to_docs_ops.rs`.
   El converter reporta `lossy_conversions` para elementos no soportados
   (tablas, math, footnotes) en lugar de fingir éxito.
3. **Co-edit safety.** Antes de cada escritura, comparamos la revisión
   actual del doc contra la cursor guardada por `agent_session_id` en
   postgres. Si un humano editó en medio, devolvemos
   `human_changes_pending` con el delta, y el agente decide si reconoce
   los cambios (`gdocs_acknowledge_human_changes`) o reintenta.

## Recommended activation

Habilita la superficie completa con un solo alias:

```json
"enabled_tools": ["gdocs"]
```

Esto expande a los 22 tools `gdocs_*` (verificado live 2026-06-09 —
todos los dispatchers llegan al LLM vía la resolución de `enabled_tools`).
Para un agente de solo-lectura usa el alias reducido:

```json
"enabled_tools": ["gdocsread"]
```

`gdocsread` excluye todos los tools que mutan el doc (replace_*,
insert_*, delete_*, append_*, apply_edits, style_*, *_named_range,
create*, share, add_tab, acknowledge_*). Quedan 6 tools de lectura
(list_tabs, read_as_markdown, read_outline, list_named_ranges, export
en modo readonly).

**Sintaxis de exclusión `!toolname`.** Igual que en otros toolkits —
verificada live 2026-06-09 con el patrón típico "todo gdocs excepto
create":

```json
"enabled_tools": ["gdocs", "!gdocs_create", "!gdocs_create_from_markdown"]
```

Resuelve a los 20 tools que quedan. Útil para el patrón realista
"user-creates-first" (ver §"Limitaciones en v1" más abajo) donde el
agente nunca debe llamar `create_*`.

Ver [40_toolkit_packages.md](40_toolkit_packages.md).

## Tool surface (28 tools)

### Creación y administración

| Tool | Qué hace |
|---|---|
| `gdocs_create` | Crea un Google Doc vacío con `title` en `parent_folder_id` (o el folder operator-default). Devuelve `{doc_id, url, revision_id, tabs}`. |
| `gdocs_create_from_markdown` | Crea un doc partiendo de un string markdown. Drive convierte nativamente; re-exportamos para detectar `lossy_conversions`. |
| `gdocs_create_from_docx` | Sube un `.docx` adjunto y lo convierte a Google Doc. **Deferido a v1.1** — el plumbing de attachment-fetcher aún no está cableado; el dispatcher devuelve `not_yet_wired` con metadata estructurada. |
| `gdocs_share` | Otorga `reader` / `commenter` / `writer` a un email. Wrapper de `drive.permissions.create`. |
| `gdocs_export` | Exporta un doc en `docx` / `pdf` / `markdown` / `txt` / `rtf` / `epub` / `odt` / `html`. v1 devuelve `{format, byte_len}`; el wrapper attachment-id queda para v1.1. |

### Multi-tab

| Tool | Qué hace |
|---|---|
| `gdocs_list_tabs` | Lista todos los tabs (incluyendo `childTabs` anidados). |
| `gdocs_add_tab` | Agrega un tab. `after_tab_id` opcional define posición. `markdown` opcional se acepta pero **el seeding queda para v1.1** (la respuesta incluye `pending_markdown_seed: true`). |

### Lectura

| Tool | Qué hace |
|---|---|
| `gdocs_read_as_markdown` | Exporta el doc (o un tab — slicing per-tab pendiente para v1.1) como `text/markdown` vía `files.export`. |
| `gdocs_read_outline` | Devuelve el outline (paragraph number, kind, preview ~80 chars, tab_id). Siempre desde un `documents.get` fresco. |
| `gdocs_list_named_ranges` | Lista todos los `namedRange` declarados, con `{named_range_id, name, paragraph_start, paragraph_end}`. |

> **⚠️ Caveat de scope en docs compartidos (hallazgo E2E 2026-06-12).** El
> refresh token OAuth de `agents@startti.co` está consentido con `drive.file`
> (per-file), no con `drive` amplio. Consecuencia: las tools que pasan por la
> **Drive API** — `gdocs_read_as_markdown` / `gdocs_export` (`files.export`) y
> `gdocs_list_documents` (`files.list`) — fallan con `403 appNotAuthorizedToFile`
> sobre un doc que el usuario **compartió** con la cuenta (drive.file solo ve
> archivos creados/abiertos por la app). En cambio, las tools que usan la
> **Docs API** — `gdocs_read_outline`, `gdocs_insert_*`, `gdocs_replace_*`,
> `gdocs_apply_edits`, etc. (`documents.get` / `documents.batchUpdate`, scope
> `documents`) — **sí funcionan** sobre docs compartidos. **Patrón
> recomendado para editar un doc compartido:** usar `gdocs_read_outline` (no
> `read_as_markdown`) para ubicar anchors, luego las tools de edición. Para
> habilitar las tools Drive-based en docs compartidos hay que re-consentir con
> `drive`/`drive.readonly` — ver BACKLOG.

### Edición quirúrgica content-addressed

| Tool | Qué hace |
|---|---|
| `gdocs_replace_text` | `find` + `replace` dentro de `scope`. Soporta `case_sensitive`, `whole_word`, `dry_run`, `confirm_many`, `occurrence`, `anchor`. |
| `gdocs_insert_after_text` | Inserta markdown justo después de un anchor (opcionalmente la `occurrence`-ésima coincidencia). |
| `gdocs_insert_before_text` | Espejo de `insert_after_text`. |
| `gdocs_insert_between` | Inserta markdown entre dos encabezados. `before_heading` omitido = hasta EOF. |
| `gdocs_insert_image_after_text` | Inserta una imagen inline justo después de un anchor. `image_url` debe ser una URL http(s) **pública** (PNG/JPEG/GIF, ≤50 MB, ≤2000 chars) — Google la baja server-side. `width_pt`/`height_pt` opcionales. **Path (i) URL-only**: insertar desde `attachment_id` (signed-URL / imagen generada) queda para v1.1. |
| `gdocs_delete_text` | Borra ocurrencias dentro de `scope`. Mismas opciones de scope/case/occurrence que `replace_text`. |
| `gdocs_replace_section` | Reemplaza todo entre un heading y el próximo mismo-o-mayor nivel (o EOF). |
| `gdocs_append_markdown` | Agrega markdown al final del doc o del tab. |

### Composición y estilo

| Tool | Qué hace |
|---|---|
| `gdocs_apply_edits` | Aplica N sub-edits (replace_text / insert_after_text / delete_text) en un solo `batchUpdate` atómico. |
| `gdocs_style_text` | Aplica un parche de estilo (`bold`, `italic`, `underline`, `strikethrough`, `font_size_pt`, `foreground`, `background`, `link`, `heading_level`) al span encontrado. |

### Named ranges

| Tool | Qué hace |
|---|---|
| `gdocs_create_named_range` | Declara un `namedRange` sobre un párrafo (v1: solo `Scope::Paragraph`). |
| `gdocs_replace_named_range` | Sobrescribe el contenido del named range vía `replaceNamedRangeContent`. |

### Co-edit guard

| Tool | Qué hace |
|---|---|
| `gdocs_acknowledge_human_changes` | Fetcha la revisión actual y la fija como cursor del agente. Úsalo después de recibir `human_changes_pending` cuando el agente decida proceder igual. |

### Discovery + permissions (Bundle 2A/2B, 2026-06-11)

| Tool | Qué hace |
|---|---|
| `gdocs_list_documents` | Drive `files.list` filtrado a `mimeType='application/vnd.google-apps.document'`. Acepta `query` (name contains), `parent_folder_id`, `modified_after` (RFC 3339), `limit`, `page_token`. Devuelve `{documents: [{doc_id, name, url, modified_time, owners[]}], next_page_token?}`. |
| `gdocs_list_permissions` | Drive `permissions.list`. Devuelve `[{permission_id, type, role, email?, display_name?}]`. Usalo ANTES de `gdocs_unshare` para obtener el `permission_id` (NO el email). |
| `gdocs_unshare` | Drive `permissions.delete`. Revoca por `permission_id`. |

### Drive Comments — humano ↔ agente messaging (Bundle 4A, 2026-06-11)

Mensajería bidireccional dentro del doc sin tocar el contenido. Útil para
flujos de revisión: el agente flagea decisiones / preguntas / blockers, el
humano resuelve desde la UI, y el agente puede listar para ver respuestas.

| Tool | Qué hace |
|---|---|
| `gdocs_add_comment` | Drive `comments.create`. Args: `doc_id`, `content`, `anchor?` (opaque Drive JSON; `None` = doc-wide). Devuelve `{comment: {comment_id, content, created_time, resolved, anchor?, author_*?}}`. |
| `gdocs_list_comments` | Drive `comments.list`. Args: `limit?`, `page_token?`, `include_resolved` (default `false`). Devuelve `{comments[], next_page_token?}`. |
| `gdocs_resolve_comment` | Drive resuelve via reply con `action: "resolve"`. Args: `doc_id`, `comment_id`, `content?` (mensaje opcional de la respuesta). |

**Workflow típico — humano pregunta, agente edita, agente resuelve:**

```
Humano: deja TODO comment: "Add stats on engagement"
↓
Agente: gdocs_list_comments({doc_id}) → ve el comment, captura comment_id
Agente: gdocs_apply_edits(...) → agrega los stats
Agente: gdocs_resolve_comment({doc_id, comment_id, content: "Added in §3"})
↓
Humano: ve el thread cerrado en la UI con la nota del agente
```

**Workflow inverso — agente pregunta antes de editar:**

```
Agente: gdocs_add_comment({doc_id, content: "@reviewer — should this cite the 2025 or 2026 study?"})
↓
Humano: responde / resuelve en la UI
↓
Agente (turn siguiente): gdocs_list_comments({include_resolved: true}) → ve la respuesta
```

**Anchors.** El `anchor` JSON solo lo genera la UI de Docs. Para v1 el
agente lo deja `None` (doc-wide) o lo pasa pass-through si llegó de un
`list_comments` previo. Pin a un range específico desde código requiere
calcular UTF-16 offsets — fuera del scope de v1.

## Auth

Dos caminos, sin configuración por-grafo:

1. **Service Account JSON** — `GOOGLE_APPLICATION_CREDENTIALS=/path/to/sa.json`.
   El email del SA debe tener acceso `writer` (o `editor`) sobre cada
   doc. Best para automatizaciones desatendidas.
2. **Application Default Credentials** — sin la env var, `yup-oauth2`
   cae a ADC: metadata server en GCP, o `gcloud auth
   application-default login` para dev local.

Scopes por defecto: `documents` + `drive`. Override vía
`COLMENA_GDOCS_SCOPES=<comma-sep>` (short names o full URLs).

> **Pivot 2026-06-09 (fix `79eae72`).** El default original era
> `documents` + `drive.file`. `drive.file` solo da acceso a archivos
> creados por la app o explícitamente compartidos vía picker — y Google
> rechaza con `appNotAuthorizedToFile` los docs simplemente compartidos
> con el SA por su email (caso de uso principal de v1). El default
> ahora es `drive` completo. Operadores con políticas estrictas pueden
> downgrade a `drive.file` vía `COLMENA_GDOCS_SCOPES` si solo trabajan
> con docs que ellos mismos crean.

### Parent folder requirement

Drive requiere un folder explícito para `gdocs_create*`. Dos formas:

1. Pasar `parent_folder_id` directamente como argumento del tool.
2. Settear `COLMENA_GDOCS_DEFAULT_PARENT_FOLDER_ID=<folder_id>` y dejar
   que el dispatcher la inyecte cuando el LLM no la provee.

Sin ninguna de las dos, el dispatcher devuelve `no_parent_folder_configured`
antes de pegarle al API.

> **Sobre ownership de archivos creados:** ver §"Limitaciones en v1 §1"
> más abajo — el SA sin Workspace no puede ownear archivos por
> `storageQuotaExceeded`. Patrón realista v1: "user-creates-first".

## Modelo content-addressed — errores tipados

El servidor resuelve cada `find` / `anchor` haciendo una búsqueda textual
sobre el contenido del doc. Tres categorías de respuesta:

| Resultado | Acción del dispatcher |
|---|---|
| 0 matches | Devuelve `text_not_found` con metadata estructurada (`find`, `scope`, sugerencias derivadas del outline). |
| 1 match | Procede a generar las requests del `batchUpdate`. |
| 2 matches con texto adyacente idéntico | Devuelve `ambiguous_match` con previews de cada candidato; el agente debe pasar `anchor` o usar `occurrence`. |
| ≥5 matches sin `confirm_many: true` | Devuelve `confirm_many_matches` con el conteo y previews para forzar una decisión explícita. |
| ≥1 match pero el agente pidió `occurrence: N` fuera de rango | Devuelve `text_not_found` con `total_matches` para que el agente reintente con un N válido. |

El `Scope` es un enum tagged en JSON:

```json
{"kind": "all"}
{"kind": "paragraph", "n": 42}
{"kind": "under_heading", "heading": "## Resumen"}
{"kind": "between_headings", "after": "## A", "before": "## B"}
{"kind": "tab", "tab_id": "t1.t2"}
```

`scope_resolver` valida el scope contra el outline cacheado por
revisión (TTL configurable vía `COLMENA_GDOCS_REVISION_CACHE_SECS`, 5s
por defecto) y devuelve `scope_crosses_boundary` si el match cruzaría
un heading que actúa como límite.

## Co-edit safety pipeline

> **v1.1 shipped 2026-06-09 — paragraph-level diff disponible.** Cuando
> hay drift, el agente recibe la lista concreta de cambios humano
> (`changes_overlapping_scope`, `changes_outside_scope`) con
> `before_text` y `after_text` por cambio. Cambios fuera del scope
> intencionado pasan como `soft_warnings` y el edit procede.
> v1 (2026-06-08) había shippeado solo con revisionId equality —
> ahora ese path es el "degraded mode" cuando no hay snapshot stored
> (instancias sin migration aplicada o docs >1 MB).

### Pipeline (v1.1)

Antes de cualquier write:

1. `agent_session_id` (estable) + `doc_id` keyean el cursor en
   `gdocs_session_state(agent_session_id, document_id, last_revision_id,
   last_snapshot_json, last_snapshot_size_bytes, last_edit_at)`.
2. El dispatcher fetcha la revisión actual del doc (cached 5s) y la
   compara con el cursor guardado.
3. Si revisiones coinciden o es first contact → proceed.
4. Si revisiones difieren y **hay un snapshot guardado** → diff
   párrafo-por-párrafo (Myers via crate `similar`) entre el snapshot
   prior y el current, particionado por overlap con el scope intencionado:
   - Algún cambio **dentro del scope** → block con
     `human_changes_pending` populado con `changes_overlapping_scope` +
     `changes_outside_scope`.
   - Todos los cambios **fuera del scope** → proceed con
     `soft_warnings` (cambios outside listados para awareness, no
     bloquean).
5. Si revisiones difieren y **no hay snapshot stored** (degraded mode)
   → block conservador con listas vacías (comportamiento v1).

El cursor + snapshot se actualizan después de **cada** write exitoso
(`replace_*`, `insert_*`, `delete_*`, `append_*`, `apply_edits`,
`style_text`, `*_named_range`). El snapshot que se persiste es el mismo
que ya hidratamos para construir `outline_snapshot` en `EditResult` —
cero API calls extra.

El agente puede:
- llamar `gdocs_acknowledge_human_changes` (fija el cursor a la
  revisión actual y captura el snapshot fresh como nuevo baseline), o
- replantear el edit con los cambios humanos visibles, o
- leer `gdocs_read_outline` / `gdocs_read_as_markdown` para más
  contexto (pero rara vez es necesario en v1.1 — el diff ya viene).

### Formato del error (v1.1)

```json
{
  "error": "human_changes_pending",
  "since": "2026-06-09T23:25:13Z",
  "changes_overlapping_scope": [
    {
      "kind": "modify",
      "paragraph": 7,
      "tab_id": "Plan",
      "preview": "Objetivo 4: ... Modificado por humano: 11:25pm",
      "before_text": "Objetivo 4: Desplegar el backend en GCP.",
      "after_text": "Objetivo 4: Desplegar el backend en GCP. Modificado por humano: 11:25pm",
      "modified_time": "2026-06-09T23:25:13Z",
      "modifying_user": null
    }
  ],
  "changes_outside_scope": [
    {
      "kind": "insert",
      "paragraph": 12,
      "tab_id": "Anexo",
      "preview": "Objetivo 5: Documentación de los endpoints",
      "before_text": null,
      "after_text": "Objetivo 5: Documentación de los endpoints",
      "modified_time": "2026-06-09T23:25:13Z",
      "modifying_user": null
    }
  ],
  "advice": "Human modified the paragraph you targeted...",
  "valid_next_moves": ["acknowledge_human_changes", "read_as_markdown", "replace_section"]
}
```

`modifying_user` queda `None` en v1.1 — no tenemos per-edit attribution
de Google. `modified_time` es la hora de detección del drift, no la
hora real del humano.

### Cap de snapshot y modo degraded

El snapshot serializado tiene un cap de **1 MB** (1,048,576 bytes) por
defecto, configurable con `COLMENA_GDOCS_MAX_SNAPSHOT_BYTES`. Si un doc
supera el cap, el snapshot se descarta (NULL) y ese (session, doc)
funciona en modo degraded — block conservador con listas vacías. Log
warn: `gdocs.snapshot.too_large` con bytes + cap + doc_id.

El modo degraded también dispara automáticamente cuando una instancia
arranca contra una DB sin la migración `20260609000000_gdocs_session_state_snapshot.sql`
aplicada — el adapter detecta la ausencia de las columnas vía
`information_schema.columns` y loguea una sola vez al boot:

```
gdocs: last_snapshot_json column missing on gdocs_session_state;
co-edit guard degrades to v1 (revisionId equality only).
Apply migration 20260609000000_gdocs_session_state_snapshot.sql
```

No crash, no data loss; solo se pierde el diff per-paragraph hasta
aplicar la migración.

### Tabla postgres (v1.1)

```sql
CREATE TABLE IF NOT EXISTS gdocs_session_state (
  agent_session_id         TEXT        NOT NULL,
  document_id              TEXT        NOT NULL,
  last_revision_id         TEXT        NOT NULL,
  last_snapshot_json       JSONB,                -- v1.1
  last_snapshot_size_bytes INTEGER,              -- v1.1
  last_edit_at             TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (agent_session_id, document_id)
);
```

Migraciones:
- `20260608000000_gdocs_session_state.sql` (v1 base)
- `20260609000000_gdocs_session_state_snapshot.sql` (v1.1 extension —
  additive `ALTER TABLE ADD COLUMN IF NOT EXISTS`).

### Limitaciones residuales (v1.1)

- **No detecta cambios solo de estilo** (bold/italic sin tocar texto).
  v1.1 compara solo texto; v1.2 agregará `style_hash` al snapshot.
- **No detecta cambios intra-paragraph carácter-perfecto.** Si dos
  ediciones humanas y agentes apuntan a partes distintas del mismo
  párrafo, v1.1 lo trata como un único Modify del párrafo entero.
  Detalle character-level queda para v1.2.
- **No atribuye a un usuario específico.** Sin per-edit log de Google,
  `modifying_user: null` siempre. Si Google expone fine-grained edit
  log en el futuro, lo wireamos directo.
- **Docs >1 MB de snapshot** funcionan en modo degraded — sin diff,
  block conservador. Configurable vía
  `COLMENA_GDOCS_MAX_SNAPSHOT_BYTES`.

## Conversión markdown ↔ Docs

`markdown_to_docs_ops.rs` cubre los elementos comunes:

| Markdown | Docs |
|---|---|
| `# H1` … `###### H6` | `paragraphStyle.namedStyleType = HEADING_1..6` |
| `**bold**`, `*italic*`, `~~strike~~` | `textStyle.bold/italic/strikethrough` |
| `` `code` `` | `textStyle.weightedFontFamily = "Roboto Mono"` |
| Inline `[link](url)` | `textStyle.link.url` |
| `- item` / `1. item` | `createParagraphBullets` con preset `BULLET_DISC_CIRCLE_SQUARE` / `NUMBERED_DECIMAL_ALPHA_ROMAN` |
| Líneas en blanco | Paragraph splits |
| `> blockquote` | `paragraphStyle.namedStyleType = NORMAL_TEXT` + indent (v1: solo indent, sin border) |

Limitaciones v1 (reportadas en `lossy_conversions`):

- **Tablas en inserts** — `gdocs_insert_*`, `gdocs_replace_section`,
  `gdocs_append_markdown`, `gdocs_apply_edits` rechazan markdown que
  contiene tablas con `invalid_args`. Tablas en
  `gdocs_create_from_markdown` sí funcionan (Drive las convierte
  nativamente), pero ediciones quirúrgicas de celdas (`set_table_cell`,
  `insert_table_row`) son v1.1.
- **Math (LaTeX `$…$`)** — pasa como texto literal.
- **Footnotes** — se omiten.
- **Imágenes inline** — se omiten (insertion de imágenes via tool
  dedicado queda para v1.1).

El converter mantiene un golden-fixture suite en
`src/libs/colmena/tests/gdocs_markdown_fixtures/` (14 fixtures) para
detectar regresiones.

## Multi-tab

Un doc puede tener múltiples tabs (Docs feature shipped 2024). Los tools
que leen aceptan `tab_id` opcional; los que escriben aceptan `tab_id`
para `append_markdown` y `add_tab`. Los content-addressed editors
(replace/insert/delete/section/style) buscan a través de todos los tabs
por defecto; restringe a un tab vía `scope: {"kind": "tab", "tab_id": "..."}`.

El `tab_id` es jerárquico: tabs anidados usan dot notation (`parent.child`).
`gdocs_list_tabs` devuelve la estructura completa.

## Hexagonal layout

- `src/libs/colmena/src/gdocs/domain/` — `DocsClient` trait,
  `Outline`, `Scope`, `EditError` enum.
- `src/libs/colmena/src/gdocs/application/` — use cases:
  `scope_resolver.rs`, `co_edit_guard.rs`, `replace_text.rs`,
  `insert.rs`, `delete_text.rs`, `replace_section.rs`, `apply_edits.rs`,
  `style.rs`, `named_range.rs`.
- `src/libs/colmena/src/gdocs/infrastructure/` — `http_client.rs`
  (REST adapter), `auth.rs`, `config.rs`, `outline_cache.rs`,
  `revision_store.rs` (postgres).
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gdocs_tools.rs` —
  los 22 dispatchers.
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/markdown_to_docs_ops.rs` —
  converter markdown → API requests + golden fixtures.

## Limitaciones en v1

Cinco hallazgos operacionales surgidos durante la live verification
2026-06-09. Documentados explícitamente porque cualquier operador real
los va a chocar el primer día.

### 1. `create_*` falla en folders personales de Gmail

Cuando el SA intenta crear un doc en un folder personal `@gmail.com`,
Google devuelve `storageQuotaExceeded` (HTTP 403). El SA tiene quota
de almacenamiento **cero**, así que aunque tenga permisos de escritura
sobre el folder, no puede ownear el archivo creado. No es bug de
colmena — es restricción de Google Drive.

**Configuraciones donde `gdocs_create*` SÍ funciona:**

| Setup | Cómo |
|---|---|
| **Shared Drive de Google Workspace** | Crear un Shared Drive en tu dominio Workspace, agregar la SA como Content Manager o Editor, usar el folder_id del Shared Drive como `parent_folder_id`. Los archivos los ownea el Shared Drive (el dominio), no la SA. |
| **Domain-wide delegation** | Configurar DWD en el admin console + grantear los scopes a la SA. La SA impersona a un usuario real; los archivos los crea "como ese usuario". |
| **OAuth user-scoped flow (v1.1)** | Pendiente. Hará que `create_*` funcione contra cualquier Gmail user con su propio refresh_token. |

**Patrón realista v1 — "user-creates-first".** Para el caso de uso
"cualquier usuario comparte un folder y el agente trabaja ahí":
1. El usuario crea el doc en su propio Drive.
2. Lo comparte con el SA como Editor.
3. Le pasa el `doc_id` al agente por chat o como input del grafo.
4. El agente nunca llama `gdocs_create*` — usa solo los tools de
   read / edit / style / append / tabs / export / share / named ranges
   sobre el doc preexistente. Useful exclusion:
   `enabled_tools: ["gdocs", "!gdocs_create", "!gdocs_create_from_markdown"]`.

### 2. `gdocs_create_from_docx` devuelve `not_yet_wired`

El dispatcher acepta el call pero responde con metadata estructurada
`{ok: false, error: "not_yet_wired", hint: "..."}`. Falta el plumbing
de attachment-fetcher (cargar bytes desde el registry). Mismo plumbing
que requiere E-T7b (gsheets xlsx). v1.1.

### 3. `gdocs_export` devuelve `byte_len`, no `attachment_id`

El export funciona contra la Drive API y devuelve el formato pedido,
pero solo reporta `{format, byte_len}` — no envuelve los bytes en un
attachment registrado. El agente puede ver el tamaño pero no compartir
el archivo downstream. Wrapping de attachment es v1.1.

### 4. `gdocs_add_tab` con `markdown` no siembra contenido

El argumento `markdown` se acepta pero NO se aplica al tab recién
creado. La response incluye `pending_markdown_seed: true` como señal
explícita. Workaround v1: crear el tab vacío y luego llamar
`gdocs_append_markdown` con el `tab_id` devuelto. v1.1 lo hará en un
paso.

### 5. ~~Co-edit guard sin diff per-paragraph~~ — shipped en v1.1 (2026-06-09)

v1.1 trae el diff estructurado vía snapshot caching en postgres. Ver
"Co-edit safety pipeline" arriba para el formato del error y el modo
degraded cuando la migración no está aplicada. Limitaciones residuales
(solo-estilo, intra-paragraph carácter-perfecto, atribución a usuario)
listadas al final de esa sección.

## Out of scope for v1 (BACKLOG)

Ver "Subsystem G v1.1" en [`BACKLOG.md`](../BACKLOG.md):

- Suggesting mode (`writeControl.suggestionsEnabled`).
- Edits quirúrgicos a celdas de tabla (`gdocs_set_table_cell`,
  `gdocs_insert_table_row`).
- Tablas markdown en inserts (requiere round-trip snapshot para
  computar índices de celda).
- Drive Comments API para mensajería humano ↔ agente in-doc.
- `gdocs_acknowledge_human_changes` enriquecido (resumen via cheap-tier
  LLM + warnings de conflictos).
- `gdocs_insert_image_after_text` (sabores attachment_id + URL).
- Plumbing de attachments para `gdocs_create_from_docx` (load bytes) y
  `gdocs_export` (register attachment).
- `gdocs_list_documents` (descubrimiento scoped a folder via Drive).
- OAuth user-scoped (hoy solo SA + ADC).
- Ejecución de Apps Script.
- Restore desde Drive Revisions (rollback).
- Math expressions en markdown (hoy pasan como `$…$` literal).

## Spec + plan

- Spec: [`docs/superpowers/specs/2026-06-08-google-docs-design.md`](../superpowers/specs/2026-06-08-google-docs-design.md)
- Plan: [`docs/superpowers/plans/2026-06-08-google-docs.md`](../superpowers/plans/2026-06-08-google-docs.md)
- Smoke graph: [`tests/graphs/agents/gdocs_smoke.json`](../../tests/graphs/agents/gdocs_smoke.json)
