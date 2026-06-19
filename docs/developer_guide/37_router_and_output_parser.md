# 37. Router & Output Parser

> **Shipped 2026-05-31.** Spec: [`docs/superpowers/specs/2026-05-31-router-and-output-parser-nodes-design.md`](../superpowers/specs/2026-05-31-router-and-output-parser-nodes-design.md). Plan: [`docs/superpowers/plans/2026-05-31-router-and-output-parser-nodes.md`](../superpowers/plans/2026-05-31-router-and-output-parser-nodes.md).

Dos nodos nuevos que cubren dos necesidades recurrentes:

- **`output_parser`** — wrapper liviano de `information_extraction` con UX pensada para encadenarlo justo después de un `llm_call` o agente. Un solo port `input`, schema inline, falla rápido si no hay input.
- **`router`** — bifurca el flujo entre N ramas nombradas. Dos modos (LLM directo / LLM extrae + reglas), subgraph opcional por rama, fail-fast sin rama default.

---

## Cuándo usar cada uno

| Necesidad | Nodo |
|---|---|
| "Tengo la salida en texto de un agente y necesito extraer `{intent, confidence}`." | `output_parser` |
| "Decidir entre `sales_agent`, `support_agent`, `billing_agent` según el mensaje del usuario." | `router` (modo `llm_direct`) |
| "El LLM debe extraer `intent` + `urgency`, y enrutar por regla `intent==sales AND urgency==high`." | `router` (modo `extract_and_route`) |
| "Decidir por un valor que ya viene estructurado de un nodo upstream (sin LLM)." | `python_node` con `output = ...` (más simple) |
| "Activar múltiples ramas en paralelo (no XOR)." | Edges independientes con `loop_status` o `python_node` — el router siempre dispara solo una rama. |

---

## `output_parser`

Recibe texto crudo en `input`, lo manda al LLM con un schema inline y devuelve el JSON parseado.

```json
{
  "type": "output_parser",
  "config": {
    "provider": "google",
    "model": "gemini-2.5-flash",
    "api_key": "${GEMINI_API_KEY}",
    "schema": {
      "intent":     { "type": "string", "required": true,  "description": "User intent: sales | support | billing" },
      "confidence": { "type": "number", "required": false, "description": "0.0 to 1.0" },
      "summary":    { "type": "string", "required": false, "description": "One-line summary" }
    },
    "instructions": "If you cannot determine the intent, use 'unknown'."
  }
}
```

**Ports:**
- `input` (default) — texto o valor crudo. Non-strings se serializan a JSON antes de mandárselo al LLM.
- Output — el JSON extraído matching el `schema`. **No** está wrappeado en `{ output: ... }`; los nodos downstream leen campos con dotted paths (`parser.intent`).

**Diferencias vs `information_extraction`:**

| | `output_parser` | `information_extraction` |
|---|---|---|
| Inputs | Un único port `input` | Múltiples `texts.{name}` |
| Schema | Inline-required (`{ type, required, description }`) | JSON Schema estándar |
| Input vacío | Hard error (`missing input`) | Silently skip |
| Mutaciones del orchestrator (`add_tasks`/`delete_tasks`) | No las soporta | Sí |

Ambos comparten internamente el motor de extracción (`util/extract_with_schema`), así que la latencia y los costos son idénticos.

---

## `router` — modo A: `llm_direct`

El LLM lee la lista de ramas (nombre + descripción) y elige una.

```json
{
  "type": "router",
  "config": {
    "mode": "llm_direct",
    "provider": "google",
    "model": "gemini-2.5-flash",
    "api_key": "${GEMINI_API_KEY}",
    "branches": [
      { "name": "sales",   "description": "User wants to buy, asks for pricing, quotes, or available products." },
      { "name": "support", "description": "User has a technical issue or asks how to use something." },
      { "name": "billing", "description": "Invoices, payments, subscriptions, refunds." }
    ]
  }
}
```

**Cuándo usarlo:** input en lenguaje natural, una única llamada al LLM (~0 latencia adicional), no necesitás los campos estructurados downstream. El LLM responde vía structured-output con un enum forzado → si alucina un nombre fuera del enum, el nodo falla con `RouterRuntimeError: llm picked unknown branch '<X>'`.

---

## `router` — modo B: `extract_and_route`

El LLM extrae un JSON contra `schema`; reglas declarativas sobre ese JSON eligen la rama.

```json
{
  "type": "router",
  "config": {
    "mode": "extract_and_route",
    "provider": "google",
    "model": "gemini-2.5-flash",
    "api_key": "${GEMINI_API_KEY}",
    "schema": {
      "intent":     { "type": "string", "required": true,  "description": "sales | support | billing" },
      "urgency":    { "type": "string", "required": false, "description": "low | medium | high" },
      "confidence": { "type": "number", "required": false, "description": "0..1" }
    },
    "branches": [
      {
        "name": "urgent_sales",
        "when": { "all": [
          { "field": "intent",  "equals": "sales" },
          { "field": "urgency", "equals": "high"  }
        ]}
      },
      { "name": "sales",   "when": { "field": "intent", "equals": "sales" } },
      { "name": "support", "when": { "field": "intent", "in": ["support", "technical"] } },
      { "name": "billing", "when": { "field": "intent", "equals": "billing" } }
    ]
  }
}
```

**Cuándo usarlo:** querés que el LLM extraiga campos estructurados (que también podés consumir downstream vía `router.<branch>.extracted.intent`), y la lógica de routing es determinista. Más auditable que modo A: la decisión se separa en "extracción" + "regla de matching".

**Orden de evaluación:** las ramas se chequean en orden de declaración. Primera que matchea gana (XOR). Por eso `urgent_sales` aparece antes que `sales` arriba — sin ese orden, `sales` matchearía primero y el `urgency: high` quedaría sin distinguir.

---

## DSL `when` — referencia

| Forma | Significado |
|---|---|
| `{ field, equals: V }` | `extracted[field] == V` (type-strict: `5 ≠ "5"`) |
| `{ field, not_equals: V }` | `extracted[field] != V` (campo ausente → true) |
| `{ field, in: [V1, V2] }` | `extracted[field] ∈ list` |
| `{ field, contains: V }` | string contiene substring V, o array contiene V |
| `{ field, gt: N }` / `lt: N` / `gte: N` / `lte: N` | comparación numérica |
| `{ field, matches: "regex" }` | regex sobre string (compilado al init) |
| `{ field, exists: true }` | campo presente y non-null |
| `{ all: [<when>, ...] }` | AND lógico |
| `{ any: [<when>, ...] }` | OR lógico |
| `{ not: <when> }` | negación |

**`field` soporta dotted paths** (ej. `"user.profile.tier"`).

**Validación al init:** el parser exige que el primer segmento del path esté declarado en `schema`. Typos como `intnt` (en vez de `intent`) fallan al cargar el grafo, no al ejecutarlo.

---

## Subgraphs por rama

Cualquier rama (en cualquier modo) puede declarar un `subgraph` opcional. Cuando esa rama gana, el router instancia un `SubGraphNode` internamente (compartiendo el mismo `SubGraphExecutorPort` del engine), lo ejecuta con la payload de la rama como input inicial, y emite el output del subgraph por el port de la rama.

```json
{
  "name": "answerable",
  "description": "User asks a question that can be answered with general knowledge.",
  "subgraph": {
    "child_graph_inline": {
      "nodes": {
        "sg_llm": { "type": "llm_call", "config": { "provider": "google", "model": "gemini-2.5-flash", "api_key": "${GEMINI_API_KEY}", "prompt": "Answer concisely: {{input}}" } },
        "sg_out": { "type": "output", "config": {} }
      },
      "edges": [ { "from": "sg_llm", "to": "sg_out" } ]
    }
  }
}
```

- `child_graph_path` y `child_graph_inline` son mutuamente excluyentes (validado al init).
- Si el subgraph **suspende**, el SUSPENDED bubblea hacia arriba a través del port de la rama (mismo comportamiento que un `SubGraphNode` standalone).
- Si el subgraph **falla**, el error se propaga con prefix `router branch '<name>': <upstream error>`.

### Qué recibe el subgrafo como input

El subgrafo arranca con su **global state** poblado con estas claves:

| Clave | Tipo | Cuándo aparece |
|---|---|---|
| `input` | mismo que el del router | Siempre. Es el texto/valor original que entró al router. |
| `extracted` | objeto JSON | Solo en Mode B. Contiene el `{ field: value }` que el LLM extrajo. |
| `__colmena_session_id` / `__colmena_agent_session_id` / `__colmena_node_id_path` / `__colmena_resume_answer` | strings | Reenviados si vinieron al router (memoria/suspend propagation). |

Dentro del subgrafo, accedés a estas claves con la sintaxis normal de Colmena. Por ejemplo, un `llm_call` puede referenciar `{{input}}` en su `prompt`, o consumir `extracted.intent` con un edge `from: "$.extracted.intent"`.

---

## Edges / wiring — cómo conectar estos nodos

### Edges del `output_parser`

El parser emite el **JSON crudo** (no envuelto en `{ output: ... }`). Hay dos formas de consumirlo desde aguas abajo:

```jsonc
"edges": [
  // Encadenar el output completo: el nodo destino recibe el objeto entero como su default input
  { "from": "llm",    "to": "parser.input" },
  { "from": "parser", "to": "log" },          // log recibe { intent: ..., confidence: ..., summary: ... }

  // Tomar un campo específico con dotted path
  { "from": "parser.intent",        "to": "next_node.category" },
  { "from": "parser.confidence",    "to": "next_node.score" }
]
```

### Edges del `router`

El router emite **múltiples puertos**, uno por rama declarada + `__decision`. Solo el puerto de la rama ganadora trae payload no-null.

```jsonc
"edges": [
  // 1. Wire el input
  { "from": "trigger.user_message", "to": "router.input" },

  // 2. Wire cada rama a su nodo aguas abajo
  //    El puerto se llama EXACTAMENTE igual que `branches[i].name`
  { "from": "router.sales",   "to": "sales_agent" },
  { "from": "router.support", "to": "support_agent" },
  { "from": "router.billing", "to": "billing_agent" },

  // 3. Opcional: tomar el __decision para audit/logging
  { "from": "router.__decision", "to": "audit_log" },

  // 4. Acceder a campos del extracted desde otro nodo (solo Mode B)
  { "from": "router.sales.extracted.urgency", "to": "priority_router.input" }
]
```

**Recordá:**
- Los puertos no elegidos emiten `null` → los nodos downstream que reciban null típicamente se skipean (depende del nodo destino).
- En Mode B, dentro de cada puerto de rama tenés `{ input, extracted }` → podés re-leer la extracción sin re-llamar al LLM.
- En Mode A, dentro de cada puerto tenés `{ input }` solamente (no hay extracción).

### Qué recibe exactamente el nodo downstream — auto-unwrap del payload

El payload de cada rama es un objeto envuelto (`{ input }` en Mode A, `{ input, extracted }` en Mode B), **no** el valor crudo. Cuando conectás `from: "router.<rama>"` a `to: "<nodo>"` **sin** especificar el puerto destino, el engine intenta desempacarlo automáticamente (*smart extraction*, `run_use_case.rs:974-982`):

1. Mira el `default_input()` del nodo destino (ej. `"input"` para `log`, `llm_call`, etc.).
2. Si el payload es un objeto **y tiene una clave con ese mismo nombre**, le pasa solo el valor interno.
3. Si **no** tiene esa clave, le pasa el objeto **entero** `{ input, extracted }`.

Como la clave del payload del router es `input`, esto define una regla simple:

| Nodo destino | `from: "router.<rama>"` directo |
|---|---|
| Su puerto por defecto se llama **`input`** (la mayoría: `log`, `llm_call`, `output_parser`, …) | ✅ El engine desempaca `input` solo |
| Su puerto por defecto **NO** es `input` (ej. `add` espera `a`/`b`) | ⚠️ Recibe el objeto `{ input, extracted }` completo bajo su puerto → casi nunca es lo que querés |
| El nodo **no declara** `default_input()` | ❌ No le entra nada |

**Patrón seguro (recomendado): sé explícito en ambos lados.** Desempacá con dotted path en `from` y nombrá el puerto en `to`:

```jsonc
{ "from": "router.sales.input", "to": "siguiente_nodo.input" }
```

Así el destino recibe el valor interno limpio sin depender de la heurística. Es justo lo que hace `router_chained.json` al encadenar dos routers:

```jsonc
{ "from": "intent_router.question.input", "to": "question_lang_router.input" }
```

> **Regla práctica:** podés conectar el router a casi cualquier nodo, pero usá `router.<rama>.input → nodo.<puerto>` para evitar los dos casos problemáticos (nodos cuyo puerto no se llama `input`, o nodos sin `default_input()`).

---

## Output: el port `__decision`

Además de los ports por rama, el router siempre emite `__decision`:

```json
{
  "selected_branch": "urgent_sales",
  "reason": "User says 'URGENTE' and asks for a quote of 50 licenses.",
  "extracted": { "intent": "sales", "urgency": "high", "confidence": 0.95 }
}
```

Útil para logging, audit trails, o como input de un nodo `log` / `task_memory_writer`. En modo A, `extracted` es `null` (no hubo extracción). En errores de routing, el `extracted` JSON va dentro del mensaje de error (no en un `__decision` parcial).

---

## Errores comunes

| Mensaje | Causa | Fix |
|---|---|---|
| `missing input — nothing to parse/route` | Tu upstream emitió `null`, `""`, `[]` o `{}`. | Asegurate que el nodo previo siempre produzca contenido, o mediá con un `python_node` que decida. A diferencia de `llm_call`, **no** se skipea silenciosamente. |
| `no branch matched. extracted: {...}` | Ninguna regla `when` matcheó. | Agregá una rama final que cubra los casos restantes (ej. `when: { field: "intent", exists: true }`), o ampliá las reglas existentes. |
| `llm picked unknown branch 'X'` | Mode A: el LLM alucinó un nombre fuera del enum. | Revisá si las descripciones son ambiguas; bajá `temperature` (default ya es 0.1); considerá pasar a mode B con un enum explícito en el schema. |
| `'when' references unknown field 'X'` | Mode B: typo en un `field` del DSL. | Corregí el typo. La validación corre al init, así que aparece antes de ejecutar. |
| `RouterConfigError: schema invalid — field 'X' has invalid type 'Y'` | Tipo no soportado en el schema inline. | Tipos válidos: `string`, `number`, `integer`, `boolean`, `array`, `object`. |

---

## Tests de integración

Siete grafos en [`tests/graphs/control_flow/`](../../tests/graphs/control_flow/):

```
output_parser_basic.json           # llm_call → output_parser → log
output_parser_review.json          # parser standalone para reviews (rating/topic/recommend/summary)
router_llm_direct.json             # mode A con 3 ramas (sales/support/billing)
router_extract_rules.json          # mode B con schema + when rules (incluye combinador 'all')
router_mode_b_sentiment.json       # mode B routing por sentiment (positive/negative)
router_with_subgraph.json          # rama con subgraph inline (LLM agent embebido)
router_chained.json                # dos routers en cascada (intent → idioma)
```

Todos requieren `GEMINI_API_KEY`. Corré uno con:

```bash
source .env
cargo run --bin dag_engine -- run tests/graphs/control_flow/router_llm_direct.json \
  --agent-session-id router_demo --include-extra-info
```

---

## Internals — quick reference

- **Archivos**: `nodes/output_parser.rs`, `nodes/router/{mod,config,when_dsl,llm_direct,extract_and_route,node}.rs`
- **Helpers compartidos**: `nodes/util/inline_schema.rs` (converter + validator), `nodes/util/extract_with_schema.rs` (LLM call + parse + validate)
- **Reuso**: `information_extraction` también delega a `extract_with_schema` desde Task 3 — los tres nodos comparten el mismo motor de extracción.
- **Wiring del executor**: el router comparte el mismo `Arc<OnceLock<SubGraphExecutorPort>>` que el `SubGraphNode`, así que el `set_subgraph_executor()` del engine los wirea a ambos con una sola llamada.
