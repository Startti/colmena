# Spec: Paragraph-level human-change diff for Google Docs co-edit guard (v1.1)

**Estado:** Aprobado — listo para implementación.
**Fecha:** 2026-06-09
**Author:** daniel@startti.co
**Subsistema:** G (Google Docs) — extiende v1 (shipped 2026-06-08).
**Backlog ref:** `docs/BACKLOG.md` → "Subsystem G v1.1" → item 2
**Plan asociado:** `docs/superpowers/plans/2026-06-09-gdocs-paragraph-diff.md`

---

## 1. Problema y goal

### El problema concreto

Hoy el co-edit guard (`gdocs/application/co_edit_guard.rs`) detecta que un
humano modificó el doc — pero **no sabe qué cambió**:

```jsonc
// Tool result actual cuando hay drift:
{
  "error": "human_changes_pending",
  "changes_overlapping_scope": [],    // ← VACÍO
  "changes_outside_scope": [],        // ← VACÍO
  "since": "2026-06-09T03:21:00Z"
}
```

El LLM tiene que llamar `gdocs_read_outline` / `gdocs_read_as_markdown`
manualmente para entender qué pasó. Eso es 1+ turn extra y compara
contra su memoria de turns previos, que puede estar truncada.

### Por qué quedó así en v1

Se pivoteó porque Drive Revisions API no devuelve edit-log per-edit para
Google Docs nativos (solo named versions). Sin per-edit attribution no
se podía atribuir cambios al humano vs al SA, ni mostrar diff.

### Goal v1.1

Cuando se detecta drift, devolver al LLM una **lista concreta de cambios
paragraph-level**, particionada por overlap con el scope intencionado, sin
ninguna API call extra. Que el LLM tenga todo el contexto en el error
mismo.

### Non-goals (parking lot)

- **Diff intra-paragraph carácter-perfecto.** Paragraph-aligned es
  suficiente; intra-paragraph queda para v1.2.
- **Detección de cambios solo de formato (bold/italic sin tocar texto).**
  v1.1 ignora style; queda para v1.2 cuando agreguemos `style_hash` al
  snapshot.
- **Atribución a usuario específico.** Sin per-edit log de Google no se
  puede; `modifying_user` permanece `None`.
- **Push/webhooks de Google al detectar cambios.** Sigue siendo
  lazy/pull en cada edit.

---

## 2. Idea central

Tras cada write exitoso del agente, persistir en Postgres el
`DocumentSnapshot` fresh (el mismo que ya hidratamos para construir el
`EditResult.outline_snapshot` — cero API calls extra). En la próxima
edición, el guard:

1. Lee snapshot prior de Postgres.
2. Compara `revision_id` con el current (fast path).
3. Si difieren → diff paragraph-level entre snapshots → partition por
   scope → bloquea o sigue.

Reusa toda la infra existente: `gdocs_session_state` table,
`RevisionStore` trait, `DocumentSnapshot` types, `co_edit_guard` flow.
No nuevos componentes — solo extensiones quirúrgicas.

---

## 3. Diseño — Data model

### 3.1 Extensión de `HumanChange`

Hoy:
```rust
pub struct HumanChange {
    pub kind: HumanChangeKind,
    pub paragraph: u32,
    pub preview: String,
    pub modified_time: chrono::DateTime<chrono::Utc>,
    pub modifying_user: Option<String>,
}
```

v1.1 — campos nuevos (additivos, backward-compatible):
```rust
pub struct HumanChange {
    pub kind: HumanChangeKind,
    pub paragraph: u32,                        // n del párrafo en current
    pub preview: String,                       // mantiene retrocompat
    pub modified_time: chrono::DateTime<chrono::Utc>,
    pub modifying_user: Option<String>,

    // NUEVOS:
    pub tab_id: Option<TabId>,
    pub before_text: Option<String>,           // None para Insert
    pub after_text: Option<String>,            // None para Delete
}
```

- `preview` queda derivado: para `Modify` = `after_text` (truncado a 120
  chars); para `Insert` = `after_text`; para `Delete` = `before_text`.
- `modifying_user` permanece `None` en v1.1 (no tenemos per-edit log).
- `modified_time` queda como `Utc::now()` al detectar drift — no es la
  hora real del humano, es la hora de detección. Mantiene el campo
  estable para v1.2 cuando Google exponga timestamp real.

### 3.2 Persistencia — `gdocs_session_state` extension

Migration nueva: `20260609000000_gdocs_session_state_snapshot.sql`

```sql
ALTER TABLE gdocs_session_state
  ADD COLUMN IF NOT EXISTS last_snapshot_json       JSONB,
  ADD COLUMN IF NOT EXISTS last_snapshot_size_bytes INTEGER;
```

- `last_snapshot_json` — `DocumentSnapshot` serializado completo
  (~5-50KB típico).
- `last_snapshot_size_bytes` — tamaño tras serialización; permite
  observabilidad + fallback a v1 cuando supera el cap.

### 3.3 Cap y fallback de tamaño

Constante: `MAX_SNAPSHOT_BYTES = 1_048_576` (1 MB).

Si el `DocumentSnapshot` serializado supera ese tamaño:
- No se persiste (`last_snapshot_json = NULL`).
- Guard se comporta como v1 (revisionId equality, listas vacías).
- Log warn: `gdocs.snapshot.too_large` con bytes + doc_id.

Default razonable; configurable vía `COLMENA_GDOCS_MAX_SNAPSHOT_BYTES`
env var si en producción se observa cap exceeded frecuente.

---

## 4. Diseño — Algoritmo de diff

### 4.1 Interfaz

```rust
// gdocs/application/diff.rs
pub fn paragraph_diff(
    prior: &DocumentSnapshot,
    current: &DocumentSnapshot,
) -> Vec<HumanChange>;
```

Pura. Sin I/O. Determinista — el orden de output es estable
(`(tab_index, paragraph_n_in_current)` ascending).

### 4.2 Algoritmo

**Paso 1 — Particionar por tab.**
Agrupar paragraphs de prior y current por `tab_id`. Los tabs que
existen en uno y no en el otro se tratan como cambios full-tab (no
crítico — multi-tab co-edit es raro).

**Paso 2 — Por cada tab, Myers diff sobre `Vec<String>` (textos).**
Usar `similar::capture_diff_slices(Algorithm::Myers, &prior_texts, &current_texts)`.

Devuelve `Vec<DiffOp>` con variants:
- `Equal { old_index, new_index, len }` → ignore (no cambio)
- `Insert { new_index, new_len, .. }` → emit `HumanChange::Insert` × new_len
- `Delete { old_index, old_len, .. }` → emit `HumanChange::Delete` × old_len
- `Replace { old_index, old_len, new_index, new_len }` →
  - Si `old_len == new_len` → emit `Modify` × old_len pareando por offset
  - Si `old_len != new_len` → emit `min(old_len, new_len)` `Modify` +
    resto `Insert` o `Delete`

**Paso 3 — Map a `HumanChange`.**
Cada op genera 1-N `HumanChange` con:
- `paragraph` = `new_index + offset + 1` (1-based, matching `ParagraphSnapshot.n`)
- `tab_id` = del tab actual
- `before_text` / `after_text` según kind
- `preview` = truncado a 120 chars

**Paso 4 — Sort estable.**
Output ordenado por `(tab_index_in_current, paragraph)` ascending. Insert
y Modify usan `paragraph` del current; Delete usa el paragraph siguiente
en current (i.e., "se borró antes de este párrafo").

### 4.3 Decisión de matching

**¿Por qué Myers sobre texto y no por `start_index` u otro estable id?**
- `start_index` (UTF-16 offset) NO es estable cross-edits — cualquier
  inserción shifte todos los siguientes.
- Google Docs no expone paragraph-stable ids vía API (las
  `paragraphId` internas no las devuelve `documents.get`).
- El text content es el matching más natural ("este es 'Objetivo 4'
  donde antes había 'Objetivo 4'").
- Myers es óptimo para LCS (longest common subsequence), maneja
  reorders moderados como Delete+Insert (aceptable — UX es "movió el
  párrafo").
- Crate `similar = "2"` ya está en `Cargo.toml`.

### 4.4 Edge case: blank lines / párrafos repetidos

Dos párrafos vacíos consecutivos son indistinguibles por texto. Myers
los matcheará 1:1 por orden — correcto la mayoría del tiempo. Caso
extremo (user borra uno de varios vacíos seguidos): puede aparecer como
"Delete del último" cuando el user borró el primero. Aceptable para v1.1
— el LLM ve el cambio neto y puede actuar.

### 4.5 Partición por scope

Tras `paragraph_diff`, partition usando `ResolvedScope` que ya devuelve
`scope_resolver`:

```rust
fn partition_by_scope(
    changes: Vec<HumanChange>,
    scope: &ResolvedScope,
) -> (Vec<HumanChange>, Vec<HumanChange>) {
    let (mut overlap, mut outside) = (Vec::new(), Vec::new());
    for c in changes {
        // Scope ya está resuelto a un rango (tab_id?, paragraph_start, paragraph_end).
        let in_scope = scope.contains_paragraph(c.tab_id.as_ref(), c.paragraph);
        if in_scope { overlap.push(c) } else { outside.push(c) }
    }
    (overlap, outside)
}
```

`ResolvedScope::contains_paragraph` es un método nuevo (helper):
- `All` → siempre `true`
- `Tab { tab_id }` → coincide tab_id, paragraph cualquiera
- `Paragraph { n }` → solo si paragraph == n y tab matches
- `UnderHeading`/`BetweenHeadings` → rango paragraph_start..=paragraph_end

---

## 5. Diseño — Co-edit guard refactor

Hoy (resumido):
```rust
let snapshot = fetch_or_cache();
let resolved = scope_resolver::resolve(scope, &snapshot)?;
let known = revisions.get(session_id, doc).await?;
match known {
    Some(k) if k != snapshot.revision_id => Err(HumanChangesPending { ..vacío.. }),
    _ => Ok(GuardOk { snapshot, resolved_scope, soft_warnings: vec![] }),
}
```

v1.1:
```rust
let snapshot = fetch_or_cache();
let resolved = scope_resolver::resolve(scope, &snapshot)?;
let (known_rev, prior_snapshot) = revisions.get_with_snapshot(session_id, doc).await?;

match known_rev {
    None => Ok(GuardOk { snapshot, resolved_scope: resolved, soft_warnings: vec![] }),
    Some(k) if k == snapshot.revision_id => {
        Ok(GuardOk { snapshot, resolved_scope: resolved, soft_warnings: vec![] })
    },
    Some(_) => {
        // Drift detected.
        let changes = match prior_snapshot {
            Some(prior) => diff::paragraph_diff(&prior, &snapshot),
            None => Vec::new(),  // Fallback v1 — snapshot was too large or migration not applied yet.
        };
        let (overlap, outside) = partition_by_scope(changes, &resolved);
        if !overlap.is_empty() {
            Err(DocsError::HumanChangesPending {
                since: Utc::now(),
                changes_overlapping_scope: overlap,
                changes_outside_scope: outside,
            })
        } else {
            // Cambios sólo fuera de scope → proceder con soft warning.
            Ok(GuardOk { snapshot, resolved_scope: resolved, soft_warnings: outside })
        }
    },
}
```

### 5.1 Cuándo se persiste snapshot

**En cada use case, después de fetch del post-write snapshot, persistir
ambos: revision_id Y snapshot.** Hoy hay 8 call sites
(`delete_text.rs`, `style.rs`, `replace_text.rs`, `replace_section.rs`
×2, `named_range.rs`, `insert.rs`, `apply_edits.rs`) que hacen
`ctx.revisions.put(session, doc, &rev)` tras `client.get` post-write.

Cambio: extender el trait a tomar `Option<&DocumentSnapshot>`:

```rust
#[async_trait]
pub trait RevisionStore: Send + Sync {
    // RENAMED & extended — old signature removed.
    async fn get_with_snapshot(
        &self,
        session_id: &str,
        doc_id: &DocumentId,
    ) -> Result<(Option<RevisionId>, Option<DocumentSnapshot>), DocsError>;

    async fn put_with_snapshot(
        &self,
        session_id: &str,
        doc_id: &DocumentId,
        rev: &RevisionId,
        snapshot: Option<&DocumentSnapshot>,
    ) -> Result<(), DocsError>;

    // Helpers de retrocompat para tests existentes:
    async fn get(&self, session_id: &str, doc_id: &DocumentId)
        -> Result<Option<RevisionId>, DocsError> {
        self.get_with_snapshot(session_id, doc_id).await.map(|(r, _)| r)
    }
    async fn put(&self, session_id: &str, doc_id: &DocumentId, rev: &RevisionId)
        -> Result<(), DocsError> {
        self.put_with_snapshot(session_id, doc_id, rev, None).await
    }
}
```

Default impls de `get`/`put` mantienen retrocompat — los tests
existentes siguen pasando sin modificar.

### 5.2 Manejo de tamaño excesivo

`put_with_snapshot` serializa, mide bytes, y si supera
`MAX_SNAPSHOT_BYTES` persiste sólo el revision_id (NULL en
`last_snapshot_json`). El log warn vive en infra layer.

---

## 6. Surface al LLM (formato del error)

### 6.1 Antes (v1)

```json
{
  "error": "human_changes_pending",
  "changes_overlapping_scope": [],
  "changes_outside_scope": [],
  "since": "2026-06-09T03:21:00Z",
  "advice": "Call read_outline to see current state"
}
```

### 6.2 Después (v1.1)

```json
{
  "error": "human_changes_pending",
  "changes_overlapping_scope": [
    {
      "kind": "modify",
      "paragraph": 7,
      "tab_id": "Plan",
      "preview": "Objetivo 4: Desplegar el backend en GCP. Modificado por humano: 11:25pm",
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
  "since": "2026-06-09T23:25:13Z",
  "advice": "Human modified the paragraph you targeted. Options: (a) acknowledge_human_changes to take latest as baseline and re-attempt your edit, (b) revise your edit to incorporate the human's change, or (c) abort if their change makes yours obsolete.",
  "valid_next_moves": ["acknowledge_human_changes", "read_as_markdown", "replace_section"]
}
```

`advice` y `valid_next_moves` ya existen en el wrapper de `DocsError` —
no se cambian aquí.

---

## 7. Migration y backward-compat

### 7.1 Migration

Archivo: `migrations/postgres/20260609000000_gdocs_session_state_snapshot.sql`

```sql
ALTER TABLE gdocs_session_state
  ADD COLUMN IF NOT EXISTS last_snapshot_json       JSONB,
  ADD COLUMN IF NOT EXISTS last_snapshot_size_bytes INTEGER;

-- Rollback:
-- ALTER TABLE gdocs_session_state DROP COLUMN IF EXISTS last_snapshot_size_bytes;
-- ALTER TABLE gdocs_session_state DROP COLUMN IF EXISTS last_snapshot_json;
```

Idempotente; no rompe runtime existente (columnas nullable). ADP debe
agregar al schema Prisma en su próximo deploy.

### 7.2 Rollout en ambientes con migration sin aplicar

Si las columnas no existen aún:
- `put_with_snapshot` → query falla con "column does not exist".

**Solución:** detección graceful. La query usa `coalesce` y si las
columnas no existen el adapter degrada a comportamiento v1. Mejor:
hacer el query INSERT/UPDATE chequeando primero existencia de columnas
al startup vía `information_schema.columns`. Si faltan, log warn una
sola vez al boot, y `snapshot` queda permanentemente None en esa
instancia.

```rust
impl PostgresRevisionStore {
    pub async fn new(pool: PgPool) -> Result<Self, DocsError> {
        let has_snapshot_col = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (\
                SELECT 1 FROM information_schema.columns \
                WHERE table_name = 'gdocs_session_state' \
                AND column_name = 'last_snapshot_json'\
            )"
        ).fetch_one(&pool).await.unwrap_or(false);

        if !has_snapshot_col {
            tracing::warn!(
                "gdocs: last_snapshot_json column missing; \
                 paragraph diff degraded to v1 (revisionId equality only). \
                 Apply migration 20260609000000_gdocs_session_state_snapshot.sql"
            );
        }
        Ok(Self { pool, has_snapshot_col })
    }
}
```

### 7.3 Compatibility con ADP

ADP debe agregar las 2 columnas al schema Prisma. Documentado en
`ADP_PRISMA_PENDING_TABLES.md` raíz (sección nueva "gdocs_session_state
v1.1 extension").

---

## 8. Testing

### 8.1 Unit tests — diff algorithm

Archivo: `gdocs/application/diff.rs` `#[cfg(test)] mod tests`.

Casos obligatorios (golden fixtures):

1. **No changes** → `vec![]`.
2. **Single Modify** prior=["a","b","c"], current=["a","B!","c"] →
   1 Modify en paragraph 2.
3. **Single Insert** end → 1 Insert en paragraph N+1.
4. **Single Insert** middle → 1 Insert con paragraph reshifted.
5. **Single Delete** → 1 Delete.
6. **Multiple separated changes** → orden estable.
7. **Replace with different lens** (3→1) → 1 Modify + 2 Delete o equiv.
8. **Multi-tab — change in 1 tab, not other** → solo cambios del tab
   afectado.
9. **Empty paragraphs duplicated** → comportamiento documented
   (deterministic, conservador).
10. **Insert + Delete del mismo párrafo** (replace en otro lugar) →
    Myers debería detectar.

### 8.2 Unit tests — partition_by_scope

5+ casos sobre `ResolvedScope::contains_paragraph`:
- All → todo overlap.
- Tab limita correctamente.
- Paragraph N → solo exacto.
- UnderHeading rango → start..=end.
- Multi-tab: cambio en tab no incluido → outside.

### 8.3 Guard tests — co_edit_guard.rs

Existentes (4 tests) se mantienen — tests con default trait impls
siguen pasando. Nuevos:

11. **drift + snapshot present + overlap** → block con `changes_*`
    populados.
12. **drift + snapshot present + no overlap** → proceed con
    `soft_warnings`.
13. **drift + snapshot None (fallback v1)** → block con listas
    vacías (same as v1).
14. **first contact + persists snapshot** → next call sees no drift.

### 8.4 RevisionStore tests

- InMemory tests roundtrip snapshot.
- Postgres tests (require DB, `#[ignore]`):
  - put_with_snapshot then get_with_snapshot returns same data.
  - size cap exceeded → snapshot=None, revision still persisted.
  - missing columns → degraded mode (test by dropping columns
    temporarily? — better: unit test of `has_snapshot_col=false` path).

### 8.5 Integration test (`#[ignore]`)

Live test against real Google Doc:
1. Create doc, write paragraph "Objetivo 1".
2. Manually edit (via Drive UI or another tool) to add "Objetivo 2".
3. Agent attempts to edit "Objetivo 1" → expect block with
   `changes_outside_scope` containing the inserted "Objetivo 2".

### 8.6 E2E graph

Adaptar `tests/graphs/agents/gdocs_phase1_build.json` +
`gdocs_phase2_continue.json` para validar:
- Phase 1: agent writes → snapshot persisted.
- Manual: human edits a paragraph.
- Phase 2: agent attempts same-scope edit → block con detalles.
- Phase 2b (post-acknowledge): agent acknowledges → proceeds.

---

## 9. Observabilidad

Tracing fields nuevos:

- `gdocs.guard.drift_detected` (bool)
- `gdocs.guard.changes_overlapping_count` (int)
- `gdocs.guard.changes_outside_count` (int)
- `gdocs.snapshot.persisted_bytes` (int)
- `gdocs.snapshot.too_large` (warn)
- `gdocs.snapshot.column_missing` (warn, una vez por instancia)
- `gdocs.diff.duration_ms` (histogram)

---

## 10. Riesgos y mitigaciones

| Riesgo | Severidad | Mitigación |
|---|---|---|
| Storage crece con # sessions activas | Bajo | Cap 1MB/snapshot; futuro GC por TTL (alineado con `last_edit_at`) — fuera de scope v1.1 |
| Myers misattribute reorders como Insert+Delete | Bajo | Documented; UX neta sigue siendo correcta |
| ADP no aplica migration → snapshot column missing | Medio | Boot warn + graceful degrade a v1 behavior; sin crash |
| `similar` crate panic en input pathológico | Muy bajo | Crate maduro (millones de descargas); cubrir con fuzzer leve si se observa |
| Diff de doc gigante (>10K paragraphs) lento | Muy bajo | Myers es O(ND); para typical docs <1s. Si supera, log warn y truncar a primeros N en respuesta |
| Race condition: 2 agentes mismo session/doc | Bajo | `INSERT ... ON CONFLICT` ya es atómico; last-writer-wins es aceptable |
| Snapshot serialization rompe BC con tests | Bajo | `DocumentSnapshot` ya `#[derive(Serialize, Deserialize)]`; tests usan `InMemoryRevisionStore` que también roundtripea via serde_json |

---

## 11. Acceptance criteria

Done cuando:

1. ✅ `cargo test --verbose` pasa local (incluido `#[ignore]` con DB).
2. ✅ `cargo clippy --all-targets -- -D warnings` clean.
3. ✅ `cargo fmt --check` clean.
4. ✅ CI verde en develop.
5. ✅ ADP worker compila contra esta colmena (sweep + cargo check).
6. ✅ Live verification (escenario §8.5) exitoso contra Google real.
7. ✅ E2E graph phase1/phase2 ejecutado y observado en `/tmp/colmena_e2e/`.
8. ✅ Doc actualizado: `45_gdocs.md` §Co-edit guard, CHANGELOG, BACKLOG
   marca item 2 done.
9. ✅ ADP doc actualizado: `ADP_PRISMA_PENDING_TABLES.md` con la nueva
   migration.
10. ✅ Backward compat verificado: instancia sin migration aplicada
    arranca con warn + degrade a v1 (test manual).

---

## 12. Costo estimado

~2-3 días subagent-driven development. Todo en colmena. Sin ADP work
bloqueante (la migration es additive; ADP la aplica cuando pueda).

---

## 13. Referencias

- Spec v1: `docs/superpowers/specs/2026-06-08-google-docs-design.md`
- Plan v1: `docs/superpowers/plans/2026-06-08-google-docs.md`
- Propuesta original v1.1: `docs/proposals/2026-06-09-gdocs-oauth-user-flow.md`
  (item 1 — referencia de format)
- Crate `similar`: https://docs.rs/similar/latest/similar/
- Algoritmo Myers: http://www.xmailserver.org/diff2.pdf
