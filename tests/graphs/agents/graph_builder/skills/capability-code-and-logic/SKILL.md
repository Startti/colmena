---
name: capability-code-and-logic
description: Use when the user needs a custom calculation/data transformation, or wants the flow to branch down different paths depending on the case. Covers python_script and router.
---

# Capacidad: cálculos a medida y bifurcar el flujo (`python_script` y `router`)

Usá esta capacidad cuando la persona quiere:

- **Un cálculo o transformación de datos a medida** que ningún nodo dedicado
  cubre (sacar un promedio, reformatear un JSON, filtrar una lista, parsear texto)
  → nodo `python_script`.
- **Que el flujo se desvíe por caminos distintos según el caso** (si es una
  consulta de ventas va por un lado, si es soporte por otro) → nodo `router`.

> Para cómo se arma y se conecta un grafo (anatomía, `nodes`, `edges`,
> triggers, puertos), mirá [[building-graphs-core]]. Esta skill solo cubre estos
> dos nodos.

---

## El nodo `python_script`

Ejecuta un fragmento de código Python dentro del grafo. Es la "puerta de escape"
más flexible de Colmena: cualquier transformación de datos que no tenga un nodo
propio se expresa en unas pocas líneas de Python.

### Cómo entran y salen los datos (regla central)

- **Las entradas se inyectan como variables.** Cada valor que llega por un edge
  se convierte en una variable global de Python con el mismo nombre. Si por un
  edge llega un campo `x` con valor `10`, dentro del código podés usar `x`
  directamente. Los objetos JSON llegan como `dict`, las listas como `list`.
- **El resultado va en la variable `output`.** El script DEBE asignar su
  resultado a una variable llamada **`output`**. Eso es lo que el nodo devuelve.
  Si el script nunca define `output`, el nodo devuelve `null`. (Usar `return` o
  `print` no sirve — el nodo busca literalmente la variable `output`.)

### Campos de `config` que importan

Estos son los nombres **exactos** de los campos (no inventar otros):

| Campo                  | Requerido | Default       | Qué es |
|------------------------|-----------|---------------|--------|
| `code`                 | Sí*       | —             | El código Python a ejecutar. Una sola expresión o un script multilínea. Debe asignar el resultado a `output`. |
| `sandbox_mode`         | No        | `"none"`      | `"none"` corre con acceso completo a Python (código de confianza). `"restricted"` valida el código antes de correr y aplica un timeout (pensado para código que escribe una IA). |
| `sandbox_timeout_secs` | No        | `10`          | Segundos máximos de ejecución. Solo se aplica cuando `sandbox_mode` es `"restricted"`. |

\* `code` puede venir por `config` o por un edge hacia el puerto `code`; al menos
una de las dos formas debe existir.

> **Claves reservadas.** `code`, `sandbox_mode` y `sandbox_timeout_secs` se
> consumen como configuración: NUNCA aparecen como variables dentro del script.
> Cualquier otra clave que llegue por un edge sí se inyecta como variable.

### El sandbox `restricted`

Cuando el código lo escribe una IA (no vos), usá `sandbox_mode: "restricted"`.
Antes de ejecutar, el nodo valida el código y solo deja pasar imports de una
lista blanca, bloqueando funciones peligrosas:

- **Imports permitidos:** `math`, `json`, `re`, `datetime`, `collections`,
  `itertools`, `functools`, `string`, `decimal`, `statistics`, `pandas`,
  `numpy`, `scipy`.
- **Builtins bloqueados:** `open`, `exec`, `eval`, `compile`, `__import__`.

Si el código viola el sandbox, el nodo devuelve un error legible
(`SandboxViolation: ...`) del que la IA puede reintentar.

> **Frontera Python → JSON.** El valor de `output` se convierte a JSON al salir
> del nodo, así que debe ser serializable: números, strings, booleanos, `None`,
> listas y **dicts con claves string**. Un `dict` con claves numéricas
> (`output = {5: 6}`) falla — coercé la clave con `str()`. Tipos como `set`,
> `bytes`, `datetime` o `Decimal` también deben convertirse explícitamente.

### Ejemplo runnable (VERBATIM): transformar un número

Grafo mínimo: recibe dos números desde arriba y devuelve un cálculo. Las
variables `x` e `y` llegan por edge y el resultado va en `output`.

```json
{
  "nodes": {
    "start": {
      "type": "mock_input",
      "config": {
        "x": 10,
        "y": 5
      }
    },
    "python_calc": {
      "type": "python_script",
      "config": {
        "code": "output = x * y + 2"
      }
    },
    "log_result": {
      "type": "log"
    }
  },
  "edges": [
    {
      "from": "start",
      "to": "python_calc"
    },
    {
      "from": "python_calc",
      "to": "log_result"
    }
  ]
}
```

Qué hace cada parte:
- `start`: emite los campos `x` (10) e `y` (5).
- El edge `start → python_calc`: ambos campos entran y se vuelven variables.
- `python_calc`: corre `output = x * y + 2` → `output = 52`.
- `log_result`: recibe el `52` y lo registra.

---

## El nodo `router`

Bifurca el flujo entre N ramas nombradas. Cada rama es un **puerto de salida**
con su propio nombre; solo el puerto de la rama elegida emite un payload no-null,
todos los demás emiten `null`. El router siempre dispara **una sola** rama (XOR)
y falla rápido si ninguna aplica (no hay rama default).

Tiene dos modos, controlados por el campo `mode`:

| `mode`               | Cómo decide |
|----------------------|-------------|
| `"llm_direct"`       | La IA lee el `name` + `description` de cada rama y elige una directamente. |
| `"extract_and_route"`| La IA primero extrae un JSON contra un `schema`, y después reglas declarativas `when` eligen la rama (en orden de declaración, gana la primera que matchea). |

### Campos de `config` que importan

| Campo      | Requerido | Qué es |
|------------|-----------|--------|
| `mode`     | Sí        | `"llm_direct"` o `"extract_and_route"`. |
| `provider` | Sí        | Proveedor del modelo. `"openai"`, `"google"`, `"anthropic"`. |
| `api_key`  | Sí        | Clave del proveedor. Soporta `${VAR}`. |
| `model`    | No        | Identificador del modelo (ej. `"gemini-2.5-flash"`). |
| `schema`   | Solo modo B | Esquema inline de los campos a extraer. **Requerido** en `extract_and_route`, prohibido en `llm_direct`. |
| `branches` | Sí        | Lista de ramas. En modo A cada rama necesita `name` + `description`. En modo B cada rama necesita `name` + `when`. |
| `instructions` | No    | Reglas extra para el system message (routing en modo A, extracción en modo B). |

### El `schema` (solo modo B)

Define qué campos extrae la IA. Cada campo lleva `type`, `required` y
`description`. Tipos válidos: `string`, `number`, `integer`, `boolean`, `array`,
`object`.

```json
"schema": {
  "intent":     { "type": "string", "required": true,  "description": "sales | support | billing" },
  "urgency":    { "type": "string", "required": false, "description": "low | medium | high" },
  "confidence": { "type": "number", "required": false, "description": "0..1" }
}
```

Las reglas `when` de cada rama operan sobre esos campos. Formas comunes:
`{ "field": "intent", "equals": "sales" }`,
`{ "field": "intent", "in": ["support", "technical"] }`,
y combinadores `{ "all": [...] }` (AND) / `{ "any": [...] }` (OR).

### Cómo se conectan las ramas (aristas peladas)

El router decide **una** rama y emite un único objeto de salida con un campo
`__decision` que trae todo lo que necesitás para ramificar después:
`__decision.selected_branch` (el `name` de la rama elegida), `__decision.reason`,
y `__decision.extracted` (los campos que sacó la IA). Conectás el router al nodo
siguiente con una **arista pelada** `{ "from": "router", "to": "..." }` y ahí leés
`__decision.selected_branch` para decidir qué hacer.

> **Por qué pelada y no `router.<rama>`.** El motor no rutea por nombre de puerto:
> una arista pelada pasa el objeto completo del router (con `__decision`) al nodo
> siguiente, y desde su `config` (con `{{templates}}` en un `llm_call`, que soportan
> rutas anidadas como `{{__decision.selected_branch}}`) elegís el campo que te
> importa. La forma punteada está **prohibida** (regla dura v1.1 — ver
> [[building-graphs-core]]).

### Ejemplo runnable: bifurcar por intención

Modo `extract_and_route`: la IA extrae `intent` + `urgency`, y las reglas `when`
mandan cada caso por su rama. `urgent_sales` va primero porque gana la primera
regla que matchea. El router emite la decisión; un `llm_call` final la lee con
`{{__decision.selected_branch}}` y redacta la respuesta adecuada.

```json
{
  "nodes": {
    "trigger": {
      "type": "trigger_webhook",
      "config": {
        "path": "/test-router-extract-rules",
        "method": "POST",
        "test_payload": {
          "user_message": "URGENTE: necesito una cotización para 50 licencias antes del fin de semana."
        }
      }
    },
    "router": {
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
    },
    "responder": {
      "type": "llm_call",
      "config": {
        "provider": "google",
        "model": "gemini-2.5-flash",
        "api_key": "${GEMINI_API_KEY}",
        "system_message": "Sos el ruteo de un equipo de atención. El router ya clasificó el mensaje en la rama '{{__decision.selected_branch}}' (motivo: {{__decision.reason}}). Redactá en una frase qué área debe atenderlo y con qué prioridad.",
        "prompt": "Mensaje clasificado en la rama: {{__decision.selected_branch}}"
      }
    },
    "out": { "type": "output", "config": {} }
  },
  "edges": [
    { "from": "trigger",   "to": "router" },
    { "from": "router",    "to": "responder" },
    { "from": "responder", "to": "out" }
  ]
}
```

Qué hace cada parte:
- `trigger`: recibe el mensaje de la persona en `user_message`.
- El edge **pelado** `trigger → router`: el payload del webhook entra al router por
  su puerto de entrada por defecto (`input`); la IA lo lee para clasificar.
- `router`: la IA extrae `intent`/`urgency` y la primera regla `when` que matchea
  elige la rama. El mensaje del ejemplo (ventas + urgente) cae en `urgent_sales`.
- El edge **pelado** `router → responder`: el objeto del router (con `__decision`)
  llega al `llm_call`, que lee la rama elegida con `{{__decision.selected_branch}}`
  en su `system_message`/`prompt` y redacta la respuesta del caso.
- `responder → out`: devuelve la respuesta final.

> **Variante modo A (`llm_direct`).** Si no necesitás campos estructurados, omití
> el `schema` y poné `mode: "llm_direct"`; cada rama lleva `name` + `description`
> y la IA elige por descripción. Más simple, pero menos auditable.

---

## Para conectar todo

El detalle de cómo se nombran los puertos, cómo se escriben los `edges` y por qué,
y cómo entra el input al primer nodo está en [[building-graphs-core]]. Volvé ahí
cada vez que dudes del cableado.
