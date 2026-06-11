# Cache-safe temporal context — integration & verification plan

**Fecha:** 2026-06-11
**Estado:** ✅ SHIPPED 2026-06-11 (todas las tareas T1-T11 completas; ver CHANGELOG §29)
**Relacionado:** item 11 (provider prompt caching, CHANGELOG §20), `35_temporal_geographic_context.md`, dev guide §14.

---

## 1. Problema

El bloque **Temporal & Geographic Context** (`format_temporal_context_block`,
[`llm.rs:3287`](../../src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs))
se inyecta como **primera sección** del system message y lleva un timestamp
con **granularidad de segundos** (`%Y-%m-%dT%H:%M:%S`). El marker
`cache_control: ephemeral` de Anthropic cubre **todo** el system block, y los
3 providers cachean por **prefijo byte-idéntico**. Como el prefijo empieza con
un timestamp variable, cualquier cambio del timestamp invalida el cache de
system+tools.

### Lo que descubrimos empíricamente (2026-06-11)

Tests live con `ANTHROPIC_API_KEY` real (SSE en `/tmp/colmena_e2e/`):

| Test | Modelo | Resultado |
|---|---|---|
| Multi-iteración intra-run | sonnet-4-6 | ✅ `cache_write 2255` (iter 1) + `cache_read 1843` (iter 2) |
| 2-run cross-turn (mismo agent_session) | sonnet-4-6 | ✅ run1 `cache_write 1824`, run2 `cache_read 1824` |
| 2-run / multi-iter | haiku-4-5 | ❌ 0 cache aun a 2904 tokens |
| curl crudo (control) | sonnet vs haiku | sonnet cachea 2896; haiku NO a 2904 |

**Conclusiones:**
1. El feature de cache **funciona correctamente** end-to-end (sonnet probado).
2. **`claude-haiku-4-5` tiene un mínimo cacheable mayor a 2048 tokens** (no
   cachea ni a ~2900) — la cifra documentada de Anthropic está desactualizada
   para esta generación.
3. **El timestamp NO rompe el cache cross-turn** gracias al gate
   `if !history_exists` ([`llm.rs:2521`](../../src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs)):
   el system se construye una sola vez en turn 1 y se reusa **congelado** desde
   la memoria en turns siguientes. El prefijo se preserva → cache hit.

### El costo oculto del freeze actual

Congelar el timestamp en turn 1 implica que en conversaciones largas el modelo
ve una **hora vieja** (turn 40 sigue creyendo que es la hora del turn 1).
Hoy hay un trade-off forzado:

| | Hora fresca por turno | Cache funciona |
|---|---|---|
| Freeze (actual) | ❌ queda vieja | ✅ |
| Refresh ingenuo (timestamp cada turno) | ✅ | ❌ rompe prefijo |

### Objetivo del fix

Sacar el timestamp **fuera del prefijo cacheado** para lograr **las dos cosas
a la vez**: timestamp fresco cada turno **Y** cache intacto. Beneficio
secundario: cachear el prefijo system+tools también **cross-conversación**
(dos chats distintos que compartan config, dentro del TTL de 5 min).

---

## 2. Diseño

### 2.1 Idea central

Separar el system message en dos partes:

- **Estable** (preludes + `system_message` del usuario + tool policy) →
  se construye en turn 1, se persiste en historial, lleva el `cache_control`
  marker. **Congelado y cacheable.**
- **Volátil** (bloque temporal) → se computa **cada turno**, se inyecta
  **fuera** del prefijo cacheado, **NO** se persiste. **Fresco y no cacheado.**

### 2.2 Nuevo campo de dominio

Agregar a `LlmConfig` (additive, no-breaking):

```rust
/// Bloque que se inyecta al FINAL del system, fuera del prefijo cacheado.
/// Regenerado cada request (timestamp, etc.). Los adapters lo colocan
/// después del contenido estable: Anthropic como 2º bloque sin marker;
/// OpenAI/Gemini concatenado al final del system string.
#[serde(default, skip_serializing_if = "Option::is_none")]
pub volatile_system_suffix: Option<String>,
```

`#[serde(default)]` → filas/configs viejas deserializan sin el campo. No
rompe persistencia ni la API pública (campo opcional aditivo).

### 2.3 Cambios en `llm.rs` (assembly)

- **Mover** el cómputo de `format_temporal_context_block(...)` **fuera** del
  `if !history_exists` para que corra **cada turno**.
- En vez de `sections.push(context_block)`, setear
  `config.volatile_system_suffix = Some(context_block)`.
- El resto de `sections` (estable) sigue dentro de `if !history_exists` →
  se persiste congelado como hoy.

### 2.4 Cambios por adapter

| Adapter | Cómo coloca el suffix | Cache |
|---|---|---|
| **Anthropic** (`build_request_body`) | Si hay suffix, emitir `system` como array de 2 bloques: `[{text: estable, cache_control: ephemeral}, {text: suffix}]`. El marker cubre solo el bloque 0. | Prefijo estable cacheado; temporal libre. |
| **OpenAI** (`build_request_body`) | Concatenar el suffix al final del system content (`{estable}\n\n{suffix}`). | Prefix-cache automático cachea el estable. |
| **Gemini** (`convert_messages`) | Concatenar el suffix al final del `systemInstruction`. | Implicit cache cachea el estable. |

**Nota OpenAI/Gemini:** no tienen marker explícito; su cache es automático por
prefijo. Con el temporal al final, el prefijo estable se cachea solo. Para
ellos el campo `volatile_system_suffix` es equivalente a concatenar — pero lo
mantenemos uniforme para que los 3 adapters compartan semántica y para no
persistir el temporal en historial.

---

## 3. Tareas de implementación (ordenadas)

| # | Tarea | Archivo(s) | Riesgo |
|---|---|---|---|
| T1 | Agregar `volatile_system_suffix` a `LlmConfig` + getter/builder | `llm/domain/llm_config.rs` | bajo (aditivo) |
| T2 | Mover cómputo temporal fuera de `if !history_exists`; setear el campo cada turno; sacar la sección del `sections` estable | `dag_engine/.../nodes/llm.rs` | medio (toca assembly + persistencia) |
| T3 | Anthropic: 2-block system cuando hay suffix; marker solo en bloque 0 | `llm/infrastructure/anthropic_adapter.rs` | medio |
| T4 | OpenAI: append suffix al final del system content | `llm/infrastructure/openai_adapter.rs` | bajo |
| T5 | Gemini: append suffix al final del `systemInstruction` | `llm/infrastructure/gemini_adapter.rs` | bajo |
| T6 | Unit tests: shape de cada adapter (Anthropic 2-block; OpenAI/Gemini concat); marker solo en bloque estable | los 3 adapters | bajo |
| T7 | **Strip-on-load** del bloque `## Temporal...` del system cargado de historial (ver §5 R3) — incluido, no opcional | `llm.rs` + 1 unit test | bajo |
| T8 | E2E live **de los 3 providers**: 2-turn con temporal distinto por turno pero cache_read>0 en turn 2 (ver §4.2) | `tests/graphs/agents/` | — |
| T9 | Docs: dev guide §14 (mínimo haiku + cómo funciona el suffix), `35_temporal_*`, CHANGELOG §29 | docs | bajo |
| T10 | ADP sweep: confirmar que ADP no construye `LlmConfig` directamente (ver R1) | repo adp | bajo |
| T11 | Sweep `cargo test --verbose` + commit + push + CI | — | — |

---

## 4. Plan de verificación

### 4.1 Unit tests (T6)

- `anthropic_adapter`:
  - `volatile_suffix_emits_two_system_blocks_marker_on_first_only` — con suffix,
    `system` es array de 2; bloque 0 tiene `cache_control`, bloque 1 no.
  - `no_suffix_keeps_single_marked_system_block` — sin suffix, comportamiento
    actual (1 bloque con marker).
- `openai_adapter`: `volatile_suffix_appended_after_stable_system`.
- `gemini_adapter`: `volatile_suffix_appended_to_system_instruction`.

### 4.2 E2E live — la prueba decisiva, **los 3 providers** (T8)

Tres grafos hermanos, mismo shape, distinto provider/modelo. Cada uno con
system >1024 tok (sonnet/openai) o el mínimo del modelo, `agent_session_id`
estable. Correr **2 turnos con ≥3s de gap** para forzar timestamps distintos:

| Grafo | provider / modelo | Campo de stats | Mín. cacheable |
|---|---|---|---|
| `provider_cache_temporal_anthropic_e2e.json` | anthropic / `claude-sonnet-4-6` | `cache_read_tokens` / `cache_write_tokens` | 1024 |
| `provider_cache_temporal_openai_e2e.json` | openai / `gpt-4o` (o `gpt-4.1`) | `cache_read_tokens` (de `cached_tokens`) | 1024 |
| `provider_cache_temporal_gemini_e2e.json` | google / `gemini-2.5-flash` | `cache_read_tokens` (de `cachedContentTokenCount`) | 1024 (flash) |

```bash
set -a && source .env && set +a
unset ANTHROPIC_BASE_URL   # ojo: override de shell sin /v1 da 404 (ver R4)
for G in anthropic openai gemini; do
  SESS=temporal_${G}_$(date +%s)
  GRAPH=tests/graphs/agents/provider_cache_temporal_${G}_e2e.json
  ./target/release/dag_engine run $GRAPH --agent-session-id $SESS --include-extra-info > /tmp/colmena_e2e/${G}_t1.sse
  sleep 3
  ./target/release/dag_engine run $GRAPH --agent-session-id $SESS --include-extra-info > /tmp/colmena_e2e/${G}_t2.sse
  echo "=== $G ==="
  grep -oE '"cache_(read|write)_tokens":[0-9]+' /tmp/colmena_e2e/${G}_t{1,2}.sse
done
```

**Criterio de éxito (por provider):**
- turn 1: cache write/creation > 0.
- turn 2: **`cache_read_tokens > 0`** a pesar de que el timestamp cambió.
- Verificar ADEMÁS que el bloque temporal del system **difiere** entre t1 y t2
  (grep del timestamp en el SSE / node-start) — esto prueba que el timestamp
  se refrescó Y el cache igual funcionó (lo que hoy, pre-fix, es imposible
  simultáneamente).

**Nota sobre cobertura de los 3:** Anthropic es el único con marker explícito
(2-block); OpenAI/Gemini dependen de su prefix-cache automático. Verificar los
3 confirma que el reorder del temporal al final del system efectivamente deja
el prefijo estable cacheable en los tres mecanismos distintos.

**Reporte amigable obligatorio** (regla `feedback_graph_runs_save_and_show`):
para cada provider, presentar qué se hizo, tokens gastados (prompt + cache_read
+ cache_write por turno), y PASS/FAIL del criterio. SSE en `/tmp/colmena_e2e/`.

### 4.3 Regresión

- Re-correr el sonnet 2-run (`provider_cache_anthropic_e2e.json`) → cache_read
  sigue >0.
- Suite completa `cargo test --verbose` → 0 fail.

---

## 5. Riesgos

| # | Riesgo | Mitigación |
|---|---|---|
| R1 | **ADP construye `LlmConfig` directo** → campo nuevo podría requerir cambios. | T10: grep en `apps/service/ia/platform/{worker,api}/src/`. Campo es `Option` con `default` → casi seguro no-breaking. |
| R2 | El modelo "ignora" el temporal al estar al final del system. | Bajo — los modelos leen todo el system. El bloque es ~55 tokens y arranca con `## Temporal & Geographic Context`. Validar en el E2E que el agente puede responder la hora correctamente. |
| R3 | **Conversaciones mid-flight** persistidas CON temporal viejo en el system congelado → al inyectar el volátil nuevo, doble temporal. | **T7 (incluido):** strip-on-load del bloque `## Temporal...` del system cargado de historial. Parser que detecta el header `## Temporal & Geographic Context` y borra el bloque hasta el siguiente `\n\n---\n` o EOF antes de usar el system. + 1 unit test que pruebe el strip sobre un system con temporal horneado. |
| R4 | `ANTHROPIC_BASE_URL` exportado en shell sin `/v1` → 404 (descubierto hoy). | Documentar en el runbook del E2E: `unset ANTHROPIC_BASE_URL` o asegurar que incluya `/v1`. NO es bug de colmena. |
| R5 | OpenAI/Gemini: el temporal al final igual cambia el system string completo → ¿su prefix-cache cachea el prefijo? | Sí: ambos cachean el **longest common prefix**, no el string completo. El prefijo estable matchea. Validar con un E2E OpenAI/Gemini si se quiere cobertura de los 3. |

---

## 6. Alcance incluido (oportunista)

Aprovechando el toque al área de cache, incluir en el mismo bundle:

1. **Fix del E2E graph existente** `provider_cache_anthropic_e2e.json`:
   ya migrado a `claude-sonnet-4-6` (haiku no cacheaba); actualizar el
   `_comment` para reflejarlo.
2. **Doc del mínimo de haiku** en dev guide §14: `claude-haiku-4-5` requiere
   un prefijo cacheable empíricamente mayor a ~2900 tokens; preferir
   sonnet/opus para cachear o garantizar prefijos grandes.

---

## 7. Estimación

~2-3h de código (T1-T7) + ~1.5h de tests y E2E **×3 providers** (T6, T8) +
~30min docs (T9) + sweep (T10-T11). Total ~4.5-5h.

## 8. Decisiones (cerradas 2026-06-11)

- **Cobertura E2E: los 3 providers** (Anthropic + OpenAI + Gemini), no solo
  Anthropic. Confirma que el reorder funciona en los 3 mecanismos de cache
  (marker explícito vs prefix-cache automático). → reflejado en §4.2.
- **T7 strip-on-load: incluido**, no opcional. Evita el doble-temporal en
  conversaciones que cruzan el deploy. → reflejado en T7 + R3.
