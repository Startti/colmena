# Documents CRDT Spike — Results & Verdict

**Date:** 2026-05-31 → 2026-06-01
**Operator:** daniel@startti.co
**Spec:** [`2026-05-31-documents-crdt-spike-design.md`](2026-05-31-documents-crdt-spike-design.md)
**Plan:** [`../plans/2026-05-31-documents-crdt-spike.md`](../plans/2026-05-31-documents-crdt-spike.md)
**Branch:** `feature/docs`
**Elapsed:** ~2 days end-to-end (well under the 2-3 week budget)

---

## Verdict

**Overall:** ✅ **GO**

El camino híbrido (Univer 0.2.10 + Yrs 0.26 + IR como proyección) es viable. Los 7 criterios pasaron, varios con holgura grande. Procedemos a diseñar v1 sobre esta base.

---

## Per-criterion results

| ID | Criterion | Threshold | Measured | Verdict |
|---|---|---|---|---|
| **R1.1** | 2 tabs + WS agent convergen | <1s, los 3 idénticos | Convergencia <1s en LAN local; automated integration test `two_ws_agents_and_one_inproc_converge` pasa 5/5 corridas; convergencia manual confirmada en browser + agente Rust simultáneamente | ✅ PASS |
| **R1.2** | Univer funciona sin su collab backend | Cero requests a `@univerjs/collaboration-server` u otros endpoints externos | UN solo WebSocket abierto, apuntando a `ws://127.0.0.1:8081/yjs/<artifact>`. `@univerjs/collaboration-client` NO aparece en la cadena transitiva de imports de `@univerjs/core@0.2.10` (verificado vía esm.sh) | ✅ PASS |
| **R2.1** | Projection p50 en 1000 celdas | <50ms | **p50 = 1.38ms, p95 = 2.12ms** (debug build, 100 corridas). Release: p50 = 0.29ms, p95 = 0.39ms. **36× margen** vs threshold | ✅ PASS |
| **R2.2** | Projection logic LoC | <500 | **79 LoC** de lógica pura en `projection.rs` (sin tests). **6× margen** | ✅ PASS |
| **R2.3** | Projection sobrevive 50 ediciones random | 100% valid JSON | 52 celdas escritas vía Univer canvas → projection responde JSON válido y captura 52 entradas | ✅ PASS |
| **R5.1** | `.xlsx` visual ingestion fidelity | Diff visual aceptable | Botón "Import /spike.xlsx" parsea fixture vía SheetJS y la grilla renderiza encabezados con color, título mergeado, 250 filas de datos + total. Fórmulas no se ven calculadas (SheetJS no las evalúa) — limitación conocida, NO bloqueante para spike | ✅ PASS (con caveat formula-eval para v1) |
| **R5.2** | Projection captura valores | 100% no-formula correctos | **756 celdas** en projection tras import. Spot-check de valores: `A3 = "SKU-0001"`, `A4 = "SKU-0002"`. Fórmulas (D3, D4, …) llegan como `null` porque el fixture xlsx no tiene cached values y SheetJS no calcula | ✅ PASS (con caveat — ver hallazgos) |

---

## Hallazgos significativos

### 1. yrs 0.26 vs y-sync 0.4 — versiones incompatibles

`y-sync 0.4` (el crate Rust que implementa Yjs sync protocol) **pina yrs 0.17.4** internamente. Coexiste con nuestro yrs 0.26 como dos crates separados pero sus tipos no son intercambiables. Reimplementamos el wire format Yjs v1 manualmente en `yjs_protocol.rs` usando las primitivas de encoding de yrs 0.26 — ~227 LoC totales incluyendo tests.

**Implicación v1:** o nos quedamos con la reimplementación manual (estable, controlable), o esperamos a que y-sync se actualice a yrs 0.26 (riesgoso, depende del mantenedor).

### 2. yrs Subscription es `!Send`

`Doc::observe_update_v1` retorna un `Subscription` con `Arc<dyn Drop>` que no implementa `Send`. axum 0.7's `ws.on_upgrade` exige `Send` en el future. Workaround: cada conexión WS hace `std::thread::spawn` con un `current_thread` tokio runtime aislado. Funciona pero gasta ~3MB stack por conexión.

**Implicación v1:** para production con 100+ conexiones simultáneas, mejor migrar a un canal Send-safe entre observer y socket-writer task. Documentado en el código.

### 3. DocRegistry race fix (TOCTOU)

Versión inicial: `DashMap::get → DashMap::insert` (no atómico). Bajo carga concurrente dos requests al mismo artifact podían crear `Doc`s divergentes y un caller terminaba con un Arc no registrado. Code review lo flageó → cambiamos a `DashMap::entry().or_insert_with()` (atómico) con regression test de 100 iteraciones.

### 4. Univer 0.2.10 — bootstrap requiere `locales: {...}`

`new Univer({ locale: LocaleType.EN_US })` no inicializa el `LocaleService`. Sin un diccionario en `locales: { [LocaleType.EN_US]: <dict> }` los plugins UI mueren silenciosamente y la grilla queda en blanco. Los diccionarios viven en `/lib/locale/en-US.json` de cada paquete (NO `/locale/en-US` que retorna stub).

### 5. y-websocket + Yjs duplicate import

`y-websocket@1.5.4` por default trae su propio yjs bundleado → instancias duplicadas, warning "Yjs was already imported". Fix correcto: importmap mapeando `"yjs"` + URL `?external=yjs` en y-websocket. Sin el importmap, `?external=yjs` rompe el módulo entero.

### 6. Univer command bus — APIs ocultas pero descubribles

Mutation ID exacto: `"sheet.mutation.set-range-values"`. Service identifiers (`ICommandService`, `IUniverInstanceService`) son tokens DI exportados como valor + tipo desde `@univerjs/core` (patrón `@wendellhu/redi`). API de dispatch: `commandService.syncExecuteCommand(id, params)`. Subscribe: `commandService.onCommandExecuted(cb)`.

`observeDeep` (no `observe`) requerido en el inbound bridge porque los Y.Map de celdas son hijos del Y.Map de cells — `observe` no detecta cambios anidados.

### 7. Initial render replay — bug encontrado en testing

La primera versión del inbound bridge solo logueaba. La segunda dispatchaba el comando. Pero ambas solo disparaban en NUEVOS cambios — al abrir tab a un artifact que ya tenía celdas, la grilla quedaba vacía. Fix: tras attach del observer, iterar `cellsMap` una vez y dispatch single SetRangeValues con todo el estado actual.

### 8. SheetJS no evalúa fórmulas

R5.2 cumple para valores literales (string/number), pero las fórmulas (D3=`=B3*C3`, etc.) llegan a la projection como `null` porque el fixture xlsx no tiene cached values y SheetJS solo lee, no calcula. **Para v1**: o el fixture/imported workbooks deben llegar con `data_only=True` (calculated values cached), o necesitamos un evaluador de fórmulas (server-side: `formulajs`, `xlsx-calc`, o el formula engine de Univer en headless).

### 9. Seeding race antes de provider sync — corregido durante testing

Primera versión semillaba sheets ANTES de `provider.sync` → race con state del server → grilla flaky (renderiza a veces). Fix: defer toda la creación de Y.Doc structures dentro de `provider.once("sync", ...)`. Patrón: Univer arranca con `cellData: {}` estático; cualquier inicialización Y.Doc-dependiente espera.

### 10. xlsx round-trip incompleto (esperado para spike)

El spike NO implementó export `.xlsx` desde el state Yjs. El operador puede ver la grilla, editar, importar — pero no exportar. v1 lo necesita.

---

## Recommendation for v1

### Llevar al v1 sin replantear

- **Stack core:** Univer 0.2.10 + `yrs 0.26` + `axum 0.7` + custom Yjs sync v1 implementation.
- **IR projection** (`Y.Doc → IR JSON`) como contrato estable para Python helper y LLM tools — funciona, es rápido, es chico.
- **Bridge architecture** Univer ↔ Y.Doc — el patrón `commandService.onCommandExecuted` + `observeDeep` + `applyingFromYDoc` flag funciona limpio.
- **DocRegistry atomic** — base correcta para multi-tenant.

### Rediseñar / añadir en v1

1. **Subscription Send-safe** — channel + writer task para escalar concurrencia.
2. **Persistencia** — el spike vive 100% en memoria. v1 necesita: snapshots periódicos (LFS / GCS) + Yjs append-only log para resume entre reinicios + retention policy.
3. **Auth + multi-tenancy** — `artifact_id` ahora es un string libre. v1 necesita ownership, ACL contra ADP user sessions.
4. **Export `.xlsx`** — round-trip completo. Probablemente vía Univer (lo importa, lo puede exportar) o vía SheetJS reverso.
5. **LLM tools CRDT-aware** — vocabulario de ops + cómo el LLM emite cambios (¿peer Yrs in-proc? ¿tool que traduce a Y.Doc writes via canal?).
6. **Python helper** — `read_sheet_as_dataframe(artifact_id, sheet_id)` + `apply_dataframe_as_patch(...)` + helpers para diff comparativo.
7. **Diff narration para LLM** — derivar de Yjs update events un resumen natural-language de los cambios del humano para que el LLM "vea" qué cambió desde el último turn.
8. **Formula evaluation** — server-side o vía Univer headless. Decidir.
9. **Production bridges** — el outbound bridge actual solo cubre cell values. Falta: formato (fills, fonts), formulas, merged cells, charts, etc. — decidir scope.
10. **Multi-sheet support** — el spike solo expone `sheets.get(0)`. v1 necesita switch entre sheets, add/delete sheet, etc.
11. **Cleanup** — `Math.random` para tab id, no IDs estables; outbound bridge no maneja deletes; initial replay solo cubre primera sheet.

### Métricas / posicionamiento

- **Latencia de convergencia** medida en LAN local: <1s visual. Aún por validar en RTT real (>50ms).
- **Footprint:** ~12KB de fixture xlsx → 756 cells en projection → projection JSON ~30KB. Manejable.
- **Browser stack:** funciona con Chrome moderno + esm.sh. Para producción debemos considerar Vite bundle (más predecible) o seguir con CDN si es solo dev.

---

## Demo

- [x] Tested manualmente extensivamente por el operador via 2 tabs + Rust agent + curl/projection.
- [ ] Video recording — opcional, no se grabó.

---

## Anexo: stack final del spike

| Capa | Componente | Versión |
|---|---|---|
| Backend CRDT | yrs | 0.26 |
| Backend Yjs protocol | (custom, ~227 LoC) | — |
| Backend HTTP | axum | 0.7 |
| Backend WS | tokio-tungstenite | 0.29 |
| Frontend canvas | Univer | 0.2.10 |
| Frontend WS provider | y-websocket | 1.5.4 (con `?external=yjs` + importmap) |
| Frontend CRDT | yjs | 13.6.18 |
| xlsx parser | SheetJS (xlsx) | 0.18.5 |
