# Colmena — Pendientes post-Plan-A/B/C

> **Status:** Plan A + Plan B + Plan C están implementados y aprobados por los final reviewers. Este doc cataloga los follow-ups no-bloqueantes flageados durante el review, el plan de deployment (push a develop coordinado con ADP), y un audit final del estado del branch.
>
> **Scope:** solo el repo colmena. Lo que falta en ADP está en `/Users/danielgarcia/startti/adp/docs/COLMENA_PLAN_B_C_PENDING.md` (backend NestJS + frontend) y `/Users/danielgarcia/startti/adp/apps/service/ia/platform/COLMENA_PLAN_B_C_PENDING.md` (Rust services + deployment ops).

---

## 1. Audit final del branch

### Estado actual

**Branch:** `workingbranch/upload_documents_with_inline`
**Commits desde main:** 38 (de `db72350` pre-rama hasta HEAD)
**Implementación:**
- Plan A — 17 commits (foundation + capability, additive)
- Plan B — 9 commits (catalog + behavior + cleanup, breaking para ADP frontend)
- Plan C — 5 commits + 1 polish (TTL cleanup binary)
- Docs — 6 commits (specs, plans, runbooks, migration notes)

**Tests:** 946 lib tests + 3 binary tests + integration tests, **todos pasan**.
**Lints:** `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo check --all-targets` — **todos clean**.

### ¿Por qué hacer un audit final antes de pushear?

Cuando trabajaste en un branch durante mucho tiempo (semanas, decenas de commits), es fácil perder de vista detalles que te morderían en producción. El audit es la última línea de defensa antes de mergear:

1. **Detectar drift:** algún archivo se quedó en un estado intermedio que pasó tests pero no representa el end-state coherente.
2. **Validar invariantes cross-plan:** Plan A asume X, Plan B usa X+Y, Plan C asume X. Si X cambió de forma sutil, los tres pueden seguir compilando pero romper en runtime.
3. **Catch-all para items que se acumularon en los final reviews y que no fueron fixed:** hay 12 items menores que los reviewers flagearon a lo largo del trabajo — vale la pena revisar si alguno se volvió crítico al ver el branch completo.
4. **Confirmar que ADP-coordination docs están al día:** los specs de migración son lo que el equipo ADP va a usar — un drift entre el código y los docs es trampa.

### Checklist del audit (~30 min, run antes del push)

- [ ] **1.1** `git log --stat workingbranch/upload_documents_with_inline...main | grep -E "^ \w+\.rs"` — verificar que no haya archivos modificados unintentionally (debe haber sólo archivos en `src/libs/colmena/src/`, `tests/`, `docs/`, `migrations/`).

- [ ] **1.2** `cargo test --verbose` (no solo `--lib` — incluye doctests). Algunos doctests en colmena son load-bearing.

- [ ] **1.3** Smoke test de los 3 grafos E2E manualmente:
  ```bash
  source .env
  cargo run --bin dag_engine -- run tests/graphs/agents/upload_inline_to_endpoint.json --agent-session-id audit_$(date +%s)
  cargo run --bin dag_engine -- run tests/graphs/agents/forward_generated_artifact.json --agent-session-id audit_$(date +%s)
  cargo run --bin dag_engine -- run tests/graphs/agents/upload_signed_url_to_endpoint.json --agent-session-id audit_$(date +%s)
  ```
  **Esperado:** los tres corren sin errores. (Pueden requerir API keys reales si los graphs llaman a un LLM — verificar primero qué necesitan.)

- [ ] **1.4** Smoke test del binario `attachment_gc`:
  ```bash
  source .env
  cargo run --bin attachment_gc -- --dry-run --ttl-days 365  # nada debe matchear con TTL tan alto
  ```
  **Esperado:** logs `gc.start`, `gc.end` con `total_deleted=0`. Si encuentra rows, algo está mal (deben haber sido limpiadas hace tiempo).

- [ ] **1.5** `grep -rn "TODO\|FIXME\|XXX" src/libs/colmena/src/ --include="*.rs" | grep -v "//\s*TODO(plan-a-opt)" | grep -v "test"` — confirmar que no quedaron TODOs nuevos sin tag.

- [ ] **1.6** Confirmar que los specs/plans están sincronizados con la implementación:
  - `docs/superpowers/specs/2026-05-25-attachment-uniform-resolution-design.md` — las 10 decisiones (D1-D10) están todas implementadas.
  - `docs/superpowers/plans/2026-05-25-attachment-uniform-resolution-plan-a.md` — 13 tasks marked done.
  - `docs/superpowers/plans/2026-05-25-attachment-uniform-resolution-plan-b.md` — 9 tasks marked done.
  - `docs/superpowers/plans/2026-05-25-attachment-uniform-resolution-plan-c.md` — 5 tasks marked done.

  **Por qué:** los plans son útiles para reconstruir el contexto. Si quedaron tasks "in progress" en los plan files, alguien que los lea más adelante se va a confundir.

- [ ] **1.7** Audit ADP migration docs:
  - `docs/superpowers/specs/2026-05-25-adp-migration-detailed.md` existe.
  - `/Users/danielgarcia/startti/adp/docs/COLMENA_PLAN_B_C_PENDING.md` existe.
  - `/Users/danielgarcia/startti/adp/apps/service/ia/platform/COLMENA_PLAN_B_C_PENDING.md` existe.
  - Las versiones coinciden (el branch ADP no tiene cambios commiteados que rompan algo de los specs).

---

## 2. Follow-ups no-bloqueantes (flageados por final reviewers)

Estos items NO bloquean merge pero son deuda técnica real. Los listo por prioridad y plan de origen.

### 2.A — Polish menor (~1-2 horas total)

| # | Item | Plan | Esfuerzo | Por qué hacerlo |
|---|---|---|---|---|
| 1 | Agregar `///` doc comments a `AttachmentResolveError` y sus variantes | A | 10 min | Convención del proyecto. Hoy es el único enum público sin doc comments. Reviewer flagged como "important issue". |
| 2 | Agregar `///` doc comment a `AttachmentStreamResolverImpl::new` | A | 5 min | Misma razón. Public constructor sin doc. |
| 3 | Agregar `///` doc comment a `render_catalog` | A | 5 min | Función pública. |
| 4 | Documentar que el marker text de `load_attachment` es deliberadamente prosa (no estructurado) en código + spec | B | 15 min | Reviewer flagged: si en el futuro alguien quiere parsear el marker para UI, va a romper. Agregar nota previene esto. |
| 5 | Eliminar `_resolved_files` dead parameter en `build_initial_user_message` (o documentar por qué se queda) | B | 20 min | Reviewer flagged: parámetro no usado. Si no hay plan futuro de usarlo, eliminarlo. Si sí, doc comment explicando. |
| 6 | Agregar comment en migration SQL de Plan A explicando que `last_used_at` se popula vía `touch_last_used` (no via trigger) | A | 5 min | Para futuros operadores que se pregunten por qué la columna queda NULL para rows viejas. |

**¿Por qué hacerlas?** Son cosméticas pero son **el tipo de deuda que se acumula**. Si las dejamos para "después", "después" nunca llega. 1-2 horas ahora = 0 horas de "qué quiso decir el implementer" en seis meses.

**¿Por qué NO son críticas?** Ninguna afecta correctness, performance, o seguridad. El código funciona como está.

### 2.B — Test coverage gaps (~2-3 horas total)

| # | Test que falta | Plan | Esfuerzo | Por qué importa |
|---|---|---|---|---|
| 7 | E2E test para `AttachmentResolveError::StorageKeyMissing` | A | 30 min | Hoy hay unit test en `stream_resolver_impl.rs` pero no en `attachment_uniform_resolution_test.rs`. Sin E2E, no probamos que el http_request multipart maneja este error correctamente. |
| 8 | Test de aislamiento cross-tenant (un `agent_session_id` no ve docs de otro) | A | 30 min | Unit test en resolver cubre esto pero solo a nivel registry. E2E confirmaría que el http_request multipart respeta el aislamiento. **Sin esto, una vulnerabilidad de cross-tenant podría pasar review.** |
| 9 | Test para `storage_key = None` rows en `attachment_gc` (legacy rows pre-Plan-A) | C | 30 min | El binario maneja esto con `if let Some(storage_key)` pero no hay test. Cuando se aplique a la DB real, va a haber rows así (pre-Plan-A). |
| 10 | Test para mixed-outcome batch en gc (una fila falla storage delete, otra succeeds) | C | 30 min | El comportamiento `skip on failure + total_storage_errors += 1` no está cubierto E2E. Sin test, una regresión silenciosa en este branch podría romper el retry semantics. |
| 11 | Test LLM-driven E2E (un modelo real emitiendo `$attachment:<id>`) | A+B | 1-2 horas | Plan A Task 12 + Plan B Task 7 explícitamente no incluyeron esto (deterministic path). Sin esto, no probamos que el catálogo + las instrucciones del system prompt son suficientes para que el modelo construya el placeholder correctamente. Caro de mantener (necesita API keys, cuesta tokens) pero es la prueba real. |

**¿Por qué hacerlas (o no)?**

- **Items 7, 9, 10:** son cheap insurance contra bugs futuros. Cada uno es un test localizado de ~30 líneas.
- **Item 8 (cross-tenant):** este es el más importante de los gaps. Sin él, un bug en la query del resolver (ej. olvidar el filter por `agent_session_id`) podría exponer docs de usuarios distintos. Vale la pena hacerlo aunque sea con `#[ignore]` si no quieren correrlo en CI.
- **Item 11 (LLM-driven):** sería great-to-have pero caro. Mi recomendación es shipear sin él y agregar más adelante si descubrimos issues en producción.

### 2.C — Refactor / optimizaciones (~2-4 horas, sesión separada)

| # | Item | Plan | Esfuerzo | Por qué considerar |
|---|---|---|---|---|
| 12 | Extraer el bloque de registration de attachments (`llm.rs:1180-1310`) a `fn register_attachments_from_files` | A/B | 1-2 horas | `LlmCallNode::execute` ahora es ~3400 líneas. Cuando alguien tiene que entender el flow de attachments, encontrar el bloque correcto es difícil. Reviewer flagged como "minor". |
| 13 | Optimización: compartir bytes signed-URL entre upload-al-provider y upload-al-storage | A | 1 hora | Ya marcado como `TODO(plan-a-opt)` en código. Re-fetcheamos los mismos bytes dos veces. Para PDFs grandes (50+ MB) es notable. |
| 14 | Decidir si el `attachment_gc` binary debe correr `sqlx::migrate!` o fallar contra schema sin migrar | C | 30 min discussion + 5 min code | Hoy corre migrations (defensivo). El reviewer flagged: si alguien deploya el GC binary antes del dag_engine cuando hay nuevas migraciones, el GC las aplica primero. No es un problema hoy pero abre una ventana. Decisión arquitectural. |
| 15 | Renombrar `PartSpec::Attachment.storage_key` a `id` o `reference` (porque ahora se usa como `document_id`) | A/B | 30 min | Cosmetic. La semántica drift no se nota en código bien testeado pero suma fricción en code review. |

**¿Por qué considerar (o no)?**

- **Item 12 (refactor llm.rs):** valuable a largo plazo pero invasivo. No bloquea Plan A+B+C ship. Hacer en sesión separada.
- **Item 13 (shared bytes):** real perf win para attachments grandes. Vale la pena pero no es crítico hoy.
- **Item 14 (migration policy):** decisión más que código. Plan C runbook ya documenta la regla "dag_engine primero". Si querés un policy más estricto, drop the migration call y haz el binario fail loud.
- **Item 15 (rename):** trivial. Hacelo cuando estés tocando esa parte del código por otra razón.

### 2.D — Operacional / deploy

| # | Item | Cuándo | Por qué |
|---|---|---|---|
| 16 | Limpiar `_sqlx_migrations` checksum drift en DB de dev | Antes del próximo cargo test --ignored | Un par de implementers se toparon con esto durante Plan A/C. Local issue solamente; CI no se ve afectado. Pero molesta a quien quiera testear localmente. |
| 17 | Push del branch a `develop` | Cuando ADP haya hecho su trabajo o cuando decidas mergear Plan A solo (es safe) | Ver sección 3 abajo. |

---

## 3. Plan de deployment (push a develop)

### ¿Por qué necesitamos un plan?

Plan A es additive (cero breaking changes). Plan B rompe el frontend de ADP (no se renderizan imágenes generadas). Plan C requiere un endpoint nuevo en el backend NestJS. **Mergear todo de una a develop sin coordinar va a romper la ADP en producción** — no de forma catastrófica, pero las imágenes generadas dejan de mostrarse hasta que el frontend ADP migre.

Hay tres estrategias posibles:

### Estrategia 1: Merge atómico de A+B+C (coordinado con ADP)

**Cuándo:** ADP ya tiene listo el backend (nuevo endpoint + chat.service.ts migrado) y el frontend (hook + component update) detrás de un feature flag.

**Pasos:**
1. ADP confirma que su backend lista los endpoints y el feature flag está en canary (~5%).
2. Push del branch colmena a `develop`. Cloud Build del worker recompila.
3. Validar smoke test (graph que genera imagen + frontend canary la renderiza).
4. Flip del feature flag a 100% en ADP.
5. Schedulear el `attachment_gc` Cloud Run Job.

**Pros:** menos commits abiertos, fewer moving parts.
**Contras:** requiere que ADP termine antes de que colmena pushee. Bloqueo cruzado.

### Estrategia 2: Merge phased — Plan A primero, luego B+C

**Cuándo:** querés bajar Plan A YA (sin esperar a ADP) y mergear B+C cuando ADP esté listo.

**Pasos:**
1. **Hoy:** cherry-pick los commits de Plan A (los primeros 17) a una rama nueva `feature/plan-a-foundation`. Push a develop.
2. La rama original `workingbranch/upload_documents_with_inline` se queda con B+C solamente.
3. **Cuando ADP esté listo:** push de la rama remaining a develop.

**Pros:** Plan A entra antes (= bytes persistidos, catálogo visible). El equipo ADP no bloquea progreso.
**Contras:** trabajo extra de cherry-picking + posible rebase pain en el branch B+C.

### Estrategia 3: Merge atómico sin coordinación (force)

**Cuándo:** estás dispuesto a aceptar que las imágenes generadas en ADP no se rendericen por días/semanas hasta que ADP migre.

**Pasos:**
1. Push del branch entero a develop.
2. ADP rolls back via revert del Cargo.toml si necesita.

**Pros:** termina rápido del lado colmena.
**Contras:** **rompe ADP en producción.** No recomendado.

### Mi recomendación: Estrategia 1 (atómico coordinado)

Razones:
1. Plan A solo no entrega valor user-visible (es foundation). Plan A+B juntos entregan la capability completa.
2. La diferencia de tiempo entre "hoy" y "cuando ADP esté listo" es ~1-2 semanas. No es mucho.
3. Mergear de forma atómica simplifica el rollback: una sola revert si hay problemas vs múltiples reverts coordinados.

### Checklist pre-push

- [ ] **3.1** Audit completo (sección 1) corrió y todo verde.
- [ ] **3.2** ADP backend (NestJS) confirma que `POST /internal/gcs/delete` y `GET /api/attachments/:documentId/url` están deployed en staging y testean OK.
- [ ] **3.3** ADP frontend confirma feature flag canary corriendo en staging con el nuevo `useAttachmentUrl` hook.
- [ ] **3.4** Sweep en `apps/service/ia/platform/worker/src/skills/` confirma que ningún graph ADP-owned depende de autoinject (o si depende, su system_prompt fue updateado).
- [ ] **3.5** `psql "$DATABASE_URL" -c "\d conversation_attachments"` confirma que las 3 columnas de Plan A están presentes en staging.
- [ ] **3.6** Mensaje preparado en el canal de comms ADP avisando del push y del rollback plan.

### Comando para pushear

Asumiendo que vamos por Estrategia 1:

```bash
cd /Users/danielgarcia/startti/colmena
git checkout workingbranch/upload_documents_with_inline
git fetch origin
git rebase origin/develop  # resolver conflicts si los hay
cargo test --verbose && cargo clippy --all-targets -- -D warnings && cargo fmt --check
# Si todo green:
git push origin workingbranch/upload_documents_with_inline
# Crear PR contra develop vía GitHub. Squash o merge commit según convención del repo.
```

Una vez merged, el siguiente push de ADP a su develop dispara Cloud Build del worker contra el nuevo colmena.

---

## 4. Comunicación con el equipo ADP

### Lo que ADP necesita saber

1. **Hay tres docs para el equipo ADP:**
   - `/Users/danielgarcia/startti/colmena/docs/superpowers/specs/2026-05-25-adp-migration-detailed.md` (spec completo, en inglés, autoritativo).
   - `/Users/danielgarcia/startti/adp/docs/COLMENA_PLAN_B_C_PENDING.md` (checklist accionable backend NestJS + frontend, en español).
   - `/Users/danielgarcia/startti/adp/apps/service/ia/platform/COLMENA_PLAN_B_C_PENDING.md` (deployment ops Rust services + GCP infra para Plan C).

2. **Esfuerzo estimado para ADP:** ~10-12 horas total (backend NestJS + frontend), ~4-5 horas más para deployment ops Rust+GCP. Total ~14-17 horas spread over 2-3 días.

3. **El branch colmena no se mergea hasta que ADP termine su parte.** Coordinación es importante.

### Mensaje sugerido para slack/canal

> Hola equipo ADP — quiero coordinar el rollout de Plan A+B+C de colmena.
>
> **Status colmena:** la rama está lista. 38 commits implementan Plan A (foundation), Plan B (catalog + behavior, breaking para el frontend), y Plan C (TTL cleanup). 946 tests pasan.
>
> **Lo que necesitan hacer:**
> - Backend NestJS (~5-8 horas): migrar `chat.service.ts`, crear el endpoint `GET /api/attachments/:documentId/url`, agregar el endpoint `POST /internal/gcs/delete`. Detalles en `apps/api/` doc adjunto.
> - Frontend (~3-4 horas): nuevo hook `useAttachmentUrl`, modificar `ChatMessage.tsx`. Detalles en frontend doc adjunto.
> - DevOps (~4-5 horas): Cloud Build del worker post-merge + Cloud Run Job para `attachment_gc`. Detalles en `apps/service/ia/platform/` doc adjunto.
>
> **Coordinación:** ideal es shipear primero el backend + frontend behind feature flag (contra la API de colmena actual), validar, luego yo mergeo colmena develop, después flip flag a 100%, después schedulo el GC job.
>
> Avísenme cuando estén listos para arrancar.

---

## 5. Por qué tener este doc separado de los planes

Plan A, B, C son **prescriptivos**: "haz X, Y, Z en este orden". Este doc es **reflexivo**: "ya hicimos X, Y, Z; acá está lo que queda".

Razones para mantenerlos separados:
1. Los plans son inmutables (sirven como bitácora de qué decidimos). Si los modifico ahora, pierdo el contexto histórico.
2. Este doc es la "deuda viva" — se va a ir modificando a medida que vamos cerrando follow-ups o descubriendo cosas nuevas.
3. Cuando alguien lea este branch en seis meses, puede ver: "el plan dijo X, el doc de pendientes dice qué quedó sin hacer". Cuenta una historia más completa que un plan + un STATUS.md genérico.

---

## 6. Cierre

Cuando todos los items de la sección 2.A (polish) estén cerrados, los items críticos de 2.B (cross-tenant test al menos) estén cerrados, y la sección 3 (deployment) esté completa con merge a develop exitoso, este doc se puede archivar. Crear un `CHANGELOG_2026-05-25.md` o similar registrando el ship.

Lo que NO está en este doc:
- Features nuevas. Si alguien quiere agregar a Plan A+B+C, es un Plan D nuevo.
- Refactors no relacionados a attachments. Si alguien quiere limpiar `llm.rs` por otras razones, va en otro plan.
