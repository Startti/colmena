# Temporal & Geographic Context — inyección automática en `llm_call`

> **Estado:** Disponible desde 0.4.0 (implementado 2026-05-18)
> **Spec:** [docs/superpowers/specs/2026-05-12-llm-temporal-geographic-context-design.md](../superpowers/specs/2026-05-12-llm-temporal-geographic-context-design.md)
> **Plan:** [docs/superpowers/plans/2026-05-18-llm-temporal-geographic-context.md](../superpowers/plans/2026-05-18-llm-temporal-geographic-context.md)

## Por qué existe

Los modelos de lenguaje no tienen reloj. Si les preguntás "¿qué día es hoy?" o "¿qué hora es?", o el LLM alucina una fecha de su training data, o responde "no tengo acceso a información actual". Lo mismo con la ubicación y el idioma del usuario — sin contexto explícito, el modelo no puede dar respuestas localizadas.

> **Actualización 2026-06-11 (cache-safe):** el bloque ya **NO** va al inicio
> del system message. Ahora se inyecta como **suffix volátil al FINAL**, fuera
> del prefijo cacheado por los providers (campo
> `LlmConfig::volatile_system_suffix`). Esto permite que el timestamp se
> **refresque cada turno** (hora correcta en chats largos) **sin romper el
> prompt caching** del prefijo estable. Antes iba al frente y quedaba congelado
> en turn 1 para no invalidar el cache. Ver
> [§14 — Bloque temporal cache-safe](14_llm_deep_dive.md) y el spec
> [`2026-06-11-temporal-block-cache-safe-design.md`](../superpowers/specs/2026-06-11-temporal-block-cache-safe-design.md).

Esta feature inyecta automáticamente, en el `system_message` de cada `llm_call`, un bloque con:

- **Fecha y hora actuales** en formato **ISO 8601** (canónico, machine-friendly) más un echo human-readable.
- **Timezone** IANA (`America/Bogota`) con su offset UTC mostrado (`UTC-5`).
- **Location** geográfica en texto libre (`Bogotá, Colombia`).
- **Locale** BCP 47 (`es-CO`) para que el modelo elija el idioma de la respuesta.

Todo se declara una sola vez al root del graph JSON. El motor computa la hora actual a runtime desde el reloj del server.

## Estándares usados

| Campo | Estándar | Por qué |
|---|---|---|
| Datetime | **ISO 8601** (RFC 3339), `2026-05-18T10:34:00-05:00` | No-ambiguo (no hay `M/D/Y` vs `D/M/Y`), parser-friendly. Es lo que Anthropic Claude inyecta en sus system prompts; lo que recomienda la industria. |
| Timezone | **IANA TZDB** | Standard universal (Linux, JS `Intl`, todos los runtimes). |
| Locale | **BCP 47** (RFC 5646), `es-CO` | El estándar formal para "idioma + región". Usado por iOS, Android, browsers, CLDR. |
| Location | Free-text | No existe un estándar para "ubicación legible por humanos". Se deja libre porque el LLM la lee, no un parser. |

## Configuración

Tres campos opcionales al **root** del graph JSON, alongside `nodes` y `edges`:

```json
{
  "timezone": "America/Bogota",
  "location": "Bogotá, Colombia",
  "locale": "es-CO",
  "nodes": { ... },
  "edges": [ ... ]
}
```

| Campo | Tipo | Default | Notas |
|---|---|---|---|
| `timezone` | string IANA | `"America/Bogota"` | Si la string es inválida (`"Mars/Olympus"`), cae silenciosamente a `America/Bogota` Y reescribe el label en el bloque renderizado para que `(timezone, offset)` queden coherentes. |
| `location` | string free-text | `"Bogotá, Colombia"` | Sin validación. Lo que pongas se renderiza tal cual. |
| `locale` | string BCP 47 | `"es-CO"` | Sin validación. El modelo es el que interpreta. |

> **Backward compatibility.** Graphs viejos sin estos fields no cambian comportamiento — los defaults se aplican a nivel del nodo `llm_call`. Cero migraciones.

## Cómo funciona — flujo end-to-end

El diagrama de abajo muestra exactamente cómo los tres fields del JSON terminan transformados en bytes en el request al provider del LLM.

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ ① Graph JSON                                                                │
│   (lo que el graph author escribe)                                          │
│                                                                             │
│   {                                                                         │
│     "timezone": "America/Bogota",   ← IANA TZDB                             │
│     "location": "Bogotá, Colombia", ← free-text                             │
│     "locale":   "es-CO",            ← BCP 47                                │
│     "nodes": { ... },                                                       │
│     "edges": [ ... ]                                                        │
│   }                                                                         │
└─────────────────────────────────────────────────────────────────────────────┘
                                     │
                                     │ serde_json::from_str
                                     ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ ② Deserialización                                                           │
│   src/libs/colmena/src/dag_engine/domain/graph.rs                           │
│                                                                             │
│   pub struct Graph {                                                        │
│       pub nodes:    HashMap<String, NodeConfig>,                            │
│       pub edges:    Vec<Edge>,                                              │
│       #[serde(default)] pub timezone: Option<String>,  // Some("America/…") │
│       #[serde(default)] pub location: Option<String>,  // Some("Bogotá,…")  │
│       #[serde(default)] pub locale:   Option<String>,  // Some("es-CO")     │
│   }                                                                         │
│                                                                             │
│   → fields opcionales; serde default = None si la key no está en JSON.      │
└─────────────────────────────────────────────────────────────────────────────┘
                                     │
                                     │ engine.execute_stream(graph, …)
                                     ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ ③ Inyección por nodo                                                        │
│   src/libs/colmena/src/dag_engine/application/run_use_case.rs               │
│                                                                             │
│   Para CADA nodo del graph que el motor va a ejecutar:                      │
│                                                                             │
│     if let Some(tz)  = graph.timezone.as_deref() {                          │
│         inputs.insert("__colmena_timezone", Value::String(tz.to_string()));│
│     }                                                                       │
│     if let Some(loc) = graph.location.as_deref() {                          │
│         inputs.insert("__colmena_location", Value::String(loc.to_string()));│
│     }                                                                       │
│     if let Some(lc)  = graph.locale.as_deref() {                            │
│         inputs.insert("__colmena_locale",   Value::String(lc.to_string())); │
│     }                                                                       │
│                                                                             │
│   → Se inyecta en TODOS los nodos, no solo en llm_call.                     │
│   → Nodos non-LLM ignoran las keys extra (no hace nada en ellos).           │
│   → Los keys con prefijo `__` se filtran del SSE node-start event.          │
└─────────────────────────────────────────────────────────────────────────────┘
                                     │
                                     │ node.execute(inputs, ...)
                                     ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ ④ Lectura en LlmNode::execute                                               │
│   src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs               │
│                                                                             │
│   Dentro del bloque `if !history_exists` (= primer turno de la sesión):     │
│                                                                             │
│     let tz_str = inputs.get("__colmena_timezone")                           │
│         .and_then(|v| v.as_str())                                           │
│         .unwrap_or("America/Bogota");        // ← default cuando absent     │
│                                                                             │
│     let loc_str = inputs.get("__colmena_location")                          │
│         .and_then(|v| v.as_str())                                           │
│         .unwrap_or("Bogotá, Colombia");      // ← default                   │
│                                                                             │
│     let locale_str = inputs.get("__colmena_locale")                         │
│         .and_then(|v| v.as_str())                                           │
│         .unwrap_or("es-CO");                 // ← default                   │
│                                                                             │
│   → Si el graph no traía el field, igual hay default — el LLM nunca         │
│     ve un bloque "vacío".                                                   │
└─────────────────────────────────────────────────────────────────────────────┘
                                     │
                                     │ format_temporal_context_block(tz, loc, locale)
                                     ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ ⑤ Cómputo en format_temporal_context_block                                  │
│   (mismo archivo llm.rs, función helper privada)                            │
│                                                                             │
│   a. Parsear timezone_str como chrono_tz::Tz                                │
│                                                                             │
│      let (tz, tz_display) = match timezone_str.parse::<Tz>() {              │
│          Ok(tz) => (tz, timezone_str.to_string()),                          │
│          Err(_) => (Bogota_fallback, "America/Bogota".to_string()),         │
│      };                                                                     │
│                                                                             │
│      → si el string es inválido, fallback a Bogotá Y se reescribe el        │
│        label visible para que ofset y label queden consistentes.            │
│                                                                             │
│   b. Hora UTC del server proyectada a la zona local:                        │
│                                                                             │
│      let local_dt = Utc::now().with_timezone(&tz);                          │
│      // ej: 2026-05-18T10:34:00-05:00                                       │
│                                                                             │
│   c. Render ISO 8601 (canónico):                                            │
│                                                                             │
│      let iso  = local_dt.format("%Y-%m-%dT%H:%M:%S%:z").to_string();        │
│      // "2026-05-18T10:34:00-05:00"                                         │
│                                                                             │
│   d. Render human-readable (echo en paréntesis):                            │
│                                                                             │
│      let human = local_dt.format("%A, %B %-d, %Y, %-I:%M %p").to_string();  │
│      // "Sunday, May 18, 2026, 10:34 AM"                                    │
│                                                                             │
│   e. Render offset display ("UTC-5" / "UTC+5:30"):                          │
│                                                                             │
│      raw_offset = "-05:00"                                                  │
│      sign='-', hours=5, mins=0                                              │
│      → "UTC-5"   (drop ":00" cuando minutes==0)                             │
│                                                                             │
│      Para Asia/Kolkata sería "UTC+5:30".                                    │
│                                                                             │
│   f. Construir el bloque final con un format!() multi-línea.                │
└─────────────────────────────────────────────────────────────────────────────┘
                                     │
                                     │ String del bloque
                                     ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ ⑥ Bloque renderizado                                                        │
│   (literalmente los caracteres que ve el LLM)                               │
│                                                                             │
│   ## Temporal & Geographic Context                                          │
│   Current date and time: 2026-05-18T10:34:00-05:00 (Sunday, May 18, 2026,  │
│   10:34 AM)                                                                 │
│   Timezone: America/Bogota (UTC-5)                                          │
│   Location: Bogotá, Colombia                                                │
│   Locale: es-CO                                                             │
└─────────────────────────────────────────────────────────────────────────────┘
                                     │
                                     │ sections.push(context_block)
                                     ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ ⑦ Ensamble del system message                                               │
│   (sections: Vec<String> en LlmNode::execute, dentro del !history_exists)   │
│                                                                             │
│   sections[0] = context_block                  ← lo nuestro va PRIMERO      │
│   sections[1] = graph_author_system_message    ← lo que escribió el autor   │
│   sections[2] = ATTACHMENTS_SYSTEM_PRELUDE     ← si hay attachments         │
│   sections[3] = tool_use_instructions          ← si hay tools               │
│   sections[…] = …                                                           │
│                                                                             │
│   Se joinea con "\n\n---\n" entre secciones.                                │
└─────────────────────────────────────────────────────────────────────────────┘
                                     │
                                     │ LlmRequest construction
                                     ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ ⑧ Request al provider (Gemini / OpenAI / Anthropic)                         │
│                                                                             │
│   SYSTEM:                                                                   │
│   ## Temporal & Geographic Context                                          │
│   Current date and time: 2026-05-18T10:34:00-05:00 (Sunday, May 18, 2026,  │
│   10:34 AM)                                                                 │
│   Timezone: America/Bogota (UTC-5)                                          │
│   Location: Bogotá, Colombia                                                │
│   Locale: es-CO                                                             │
│                                                                             │
│   ---                                                                       │
│                                                                             │
│   You are a helpful local assistant. Answer using the contextual            │
│   information you have. Respond in the user's locale language.              │
│                                                                             │
│   USER:                                                                     │
│   ¿Qué fecha y hora es ahora? ¿Dónde estoy ubicado? ¿En qué idioma…?        │
│                                                                             │
│   ASSISTANT (lo que el modelo responde):                                    │
│   "La fecha y hora actual es domingo, 18 de mayo de 2026, 10:34 AM hora    │
│    local de Bogotá (UTC-5). Estás ubicado en Bogotá, Colombia, y debo      │
│    responderte en español."                                                 │
└─────────────────────────────────────────────────────────────────────────────┘
```

## Qué pasa en cada turno

| Turno | Bloque temporal | Por qué |
|---|---|---|
| **1** (sin history persistido) | Se computa y prepende a `sections[0]` | `if !history_exists` se cumple, todos los `sections.push` corren. |
| **2+** (con history persistido) | NO se re-computa | Se entra al else-branch de `if !history_exists`. El system message de turno 1 ya está en `llm_node_history` y se replay-ea. Esto significa que **el datetime se congela al primer turno** — para conversaciones cortas (típicas) está bien; para sesiones de muchas horas, conviene re-arquitectar. |

> **v1 limitation.** El timestamp NO se actualiza turno-a-turno. Es aceptable para sesiones < 24h y se documenta en el spec. Si en el futuro se necesita, conviene mover la inyección al call assembly afuera del `!history_exists` guard.

## Cómo se computa la hora

- **Fuente de verdad:** `Utc::now()` (reloj del server donde corre `dag_engine`).
- **Proyección:** `Utc::now().with_timezone(&tz)` donde `tz` es el `chrono_tz::Tz` parseado del string IANA.
- **No depende del reloj del cliente**, del browser, ni de variables de entorno tipo `TZ`. Sólo del reloj del proceso engine.

Si el server está en UTC pero el graph dice `timezone: "Europe/Madrid"`, el modelo ve la hora de Madrid correctamente.

## Comportamiento ante input inválido

| Input | Resultado |
|---|---|
| `timezone` omitido | Default `"America/Bogota"` |
| `timezone: "Mars/Olympus"` | Fallback a `America/Bogota` + label reescrito a `America/Bogota` (no se muestra el string inválido) |
| `timezone: ""` (vacío) | Fallback a `America/Bogota` (parse falla con string vacío) |
| `location` omitido | Default `"Bogotá, Colombia"` |
| `location: ""` (vacío) | Se renderiza `Location: ` (vacío detrás del `:`) — sin validación |
| `locale` omitido | Default `"es-CO"` |
| `locale: "gibberish"` | Se renderiza tal cual; el modelo decide qué hacer |

## Half-hour offsets

`chrono-tz` maneja correctamente offsets sub-hora. Para `Asia/Kolkata`:

```
Current date and time: 2026-05-18T22:04:00+05:30 (Sunday, May 18, 2026, 10:04 PM)
Timezone: Asia/Kolkata (UTC+5:30)
```

El formato `UTC+5:30` se construye preservando los minutos cuando son distintos de cero (no se renderiza como `UTC+5.5` ni como `UTC+5:30:00`).

## Cómo testearlo

### Smoke graph

`tests/graphs/agents/llm_temporal_context_test.json` — pregunta al modelo qué fecha/hora es, dónde está, y en qué idioma debe responder:

```bash
set -a; source .env; set +a
cargo run --bin dag_engine -- run tests/graphs/agents/llm_temporal_context_test.json
```

Verificá que la respuesta del modelo:
- Mencione una fecha plausible (no de su training data) — debe ser hoy.
- Mencione Bogotá / Colombia.
- Esté en español.

### Unit tests

6 tests en `temporal_context_helper_tests` (mismo `llm.rs`):

```bash
cargo test -p colmena_dag_engine --lib temporal_context_helper_tests
```

Cubren: header canónico, ISO 8601 shape, human echo entre paréntesis, 3 líneas Timezone/Location/Locale, half-hour offset (Asia/Kolkata UTC+5:30), fallback coherente para IANA inválido.

### Engine deserialization tests

3 tests en `temporal_context_tests` (en `graph.rs`):

```bash
cargo test -p colmena_dag_engine --lib temporal_context_tests
```

Cubren: graph sin los fields parsea con None, graph con los 3 fields parsea correctamente, graph con un solo field (locale) parsea OK.

## Configuración cero (lo que pasa si no toco nada)

Si tu graph no declara `timezone`/`location`/`locale`, igualmente vas a ver el bloque renderizado con los defaults:

```
## Temporal & Geographic Context
Current date and time: 2026-05-18T10:34:00-05:00 (Sunday, May 18, 2026, 10:34 AM)
Timezone: America/Bogota (UTC-5)
Location: Bogotá, Colombia
Locale: es-CO

---

[tu system_message]
```

Esto es intencional: queremos que TODA conversación tenga contexto temporal mínimo, aunque el graph author no haga nada. Si querés desactivarlo no podés — la spec no expone un flag para apagarlo (los defaults son siempre ≠ vacío).

## Cómo cambiar los defaults globalmente

Hoy no hay un override central — los defaults están hardcodeados en `format_temporal_context_block` y en la lectura de inputs. Si tu deployment opera primarily en Madrid, no Bogotá, dos opciones:

1. **Pasá los 3 fields en cada graph JSON** que ADP genere. Es lo más explícito.
2. **Cambiá los defaults en el código** (`unwrap_or("Europe/Madrid")` etc.) — requiere fork del engine.

Una futura iteración podría exponer `[temporal_context_defaults]` en un config del engine para hacer esto sin tocar código.

## Limitaciones conocidas (v1)

1. **Bloque no se refresca turno-a-turno.** Solo el primer turno computa el datetime; turnos siguientes lo reciben del history persistido. Para sesiones de > algunas horas, el modelo verá un timestamp viejo. Ver "Qué pasa en cada turno" arriba.
2. **No hay locale-aware formatting del `human` echo.** `chrono` default es inglés (`Sunday, May 18, 2026`). Si querés `Domingo, 18 de mayo de 2026`, requiere `chrono`'s `unstable-locales` feature + lookup de locale → chrono `Locale` enum. Fuera de scope v1.
3. **No hay override per-nodo.** Si querés que un `llm_call` específico mienta sobre la timezone (ej. simular respuestas como si fuera de otra zona), no podés vía config — sería una feature explícita "per-node temporal context override".
4. **No hay locale-aware validation.** Cualquier string en `locale` pasa al modelo. Si pasás `"klingon"`, el LLM hace lo que pueda.

## Archivos relevantes

- Spec: [docs/superpowers/specs/2026-05-12-llm-temporal-geographic-context-design.md](../superpowers/specs/2026-05-12-llm-temporal-geographic-context-design.md)
- Plan: [docs/superpowers/plans/2026-05-18-llm-temporal-geographic-context.md](../superpowers/plans/2026-05-18-llm-temporal-geographic-context.md)
- Schema: [docs/node_configurations.json](../node_configurations.json) → `graph_root_fields`
- Código:
  - `src/libs/colmena/src/dag_engine/domain/graph.rs` — struct
  - `src/libs/colmena/src/dag_engine/application/run_use_case.rs` — inyección
  - `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs` — `format_temporal_context_block` + wiring
- Test graph: `tests/graphs/agents/llm_temporal_context_test.json`
