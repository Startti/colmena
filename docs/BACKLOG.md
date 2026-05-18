# Backlog — Future work / parked items

> **Propósito:** Listar features identificados, especificados o solicitados que **no están en el roadmap activo**. Cada entrada tiene un trigger explícito ("¿cuándo retomamos esto?") para evitar que algo se quede olvidado o se construya prematuramente.

Si vas a empezar a trabajar en algo de acá, sacalo de esta lista y agregalo al changelog del mes correspondiente. Si descartás definitivamente un item, marcalo `~~tachado~~` y dejá una nota explicando por qué.

---

## 1. `data:` (base64 inline) — auto-summary v2 con tee del upload stream

**Origen:** Limitación #1 del feature de auto-summary, post-fix de path/data del 2026-05-16.

**Problema.** Cuando un archivo entra al `llm_call` como `data:` (base64 inline) sin un `url` o `path` que lo respalde, los bytes los consume `upload_streaming` al subirlo al provider y no se retienen en memoria. La fila igual se registra en `conversation_attachments` con `AttachmentSource::Inline` (post el fix de path/data), pero el auto-summary salta esa fila porque no puede re-leer los bytes para extraer texto. El catálogo del `load_attachment` muestra solo `filename` como label — sin descripción generada.

**Workaround actual.** El caller pasa `description` manualmente en el `files[]` entry. Los tres modos de input se comportan así:

| Input | Registración | Auto-summary | Workaround |
|---|---|---|---|
| `url:` (signed URL) | ✅ | ✅ | — |
| `path:` (disco local) | ✅ | ✅ (re-lee del disco) | — |
| `data:` (base64 inline) | ✅ | ❌ | pasar `description` manualmente |

**Por qué está parqueado.** En producción ADP usa exclusivamente signed URLs (es el flow nativo del frontend con GCS). Path-based se usa para testing local. Inline base64 sería un edge case para flows experimentales que aún no existen. Costo de fix > beneficio actual.

**Fix propuesto (v2).** Tee del stream de upload: una vez que `parse_file_entries` produce `FileSource::InlineBytes { bytes }`, dividir el flujo en dos consumers:

1. **Upload stream** → consume hacia `provider.upload_streaming()` (existing).
2. **Retention buffer** → bytes copiados a `Vec<u8>` retenido en el `SummaryTarget` para que el summary path pueda usarlos sin re-descargar.

Alternativa más simple: copiar los bytes ANTES del upload (doble memoria temporal) y pasar el `Vec<u8>` original como `inline_bytes` en el `SummaryTarget`. Para archivos < 100 MB (límite del frontend) la doble copia es aceptable.

**Acceptance criteria.**

- Un graph con `files[]` usando `data:` y SIN `description` resulta en una fila de `conversation_attachments` con `description NOT NULL` después del primer turno.
- El reader en un segundo turno (mismo `agent_session_id`) llama `load_attachment` y describe el contenido correctamente.
- Sin regresión en el flujo `url:` ni `path:`.
- Sin costo adicional de red (no re-descargar, no re-upload).

**Estimación.** ~80-120 LOC. Plan TDD similar al fix de path/data (commit chain del 2026-05-16). Dependencia: ninguna nueva.

**Cuándo retomar.**

- Cuando ADP empiece a usar uploads inline (base64) en alguna feature.
- O cuando un tercer integrador no-ADP necesite el flujo.
- O cuando el dev guide reciba quejas concretas de la limitación.

**Referencias.**

- Limitación documentada: [docs/developer_guide/31_load_attachment.md](developer_guide/31_load_attachment.md) → "Limitaciones conocidas (v1)" item #1.
- Spec del fix de path/data (precedente metodológico): [docs/superpowers/specs/2026-05-16-load-attachment-path-data-registration-issue.md](superpowers/specs/2026-05-16-load-attachment-path-data-registration-issue.md).
- Plan del fix de path/data (referencia para estructura del v2 plan): [docs/superpowers/plans/2026-05-16-load-attachment-path-data-fix.md](superpowers/plans/2026-05-16-load-attachment-path-data-fix.md).

---

## Cómo agregar un item a este backlog

Cada entrada debe tener:

- **Origen** — de dónde vino la idea (audit, conversación con stakeholder, bug report).
- **Problema** — qué duele actualmente.
- **Workaround actual** — qué tiene que hacer el usuario hoy en lugar de la solución.
- **Por qué está parqueado** — qué pesó más, prioridad o costo.
- **Fix propuesto** — boceto de la solución (1-2 párrafos), suficiente para retomar sin tener que re-pensar todo.
- **Acceptance criteria** — qué define que el fix está completo.
- **Estimación** — orden de magnitud (LOC, días, dependencias nuevas).
- **Cuándo retomar** — un trigger concreto, no "cuando haya tiempo".
- **Referencias** — links a docs/specs/plans existentes.
