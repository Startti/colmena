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

### Cómo se conectan las ramas (puertos nombrados — la EXCEPCIÓN a las aristas peladas)

El router es el **único** caso donde las aristas NO son peladas. Cada rama es un
**puerto de salida con nombre propio**, y para que el ruteo funcione tenés que
conectar cada rama por su nombre: `{ "from": "router.<rama>", "to": "..." }`.

> **Por qué punteado acá (y solo acá).** El router emite un payload no-null **solo**
> por el puerto de la rama elegida; los demás puertos emiten `null` y su rama no se
> dispara. Si conectaras el router con una arista pelada, TODAS las ramas se
> dispararían a la vez (el motor no sabría a cuál mandar el flujo). Por eso las
> ramas de un router van SIEMPRE punteadas `router.<rama>`. Esta es la única
> excepción a la regla de aristas peladas (ver [[building-graphs-core]]).

Además del puerto por rama, el router expone un puerto `router.__decision` con la
decisión completa (`selected_branch`, `reason`, y en modo B los campos extraídos).
Es **opcional**: conectalo solo si un nodo posterior necesita leer por qué se eligió
esa rama. El input del router entra por su puerto por defecto (`input`): la arista
que lo alimenta es **pelada** (`{ "from": "trigger", "to": "router" }`), o
explícita hacia `router.input` si querés ser literal.

> **Nombres de rama.** Cada `name` de rama debe ser un slug en minúsculas que matchee
> `^[a-z][a-z0-9_]{0,63}$` (empieza con letra, solo minúsculas, dígitos y `_`).

### Ejemplo runnable (VERBATIM): clasificar un mensaje y responder con el especialista

Modo `llm_direct`: la IA lee `name` + `description` de cada rama y elige una. Cada
rama va a su `llm_call` especialista por un puerto nombrado `router.<rama>`. El
mensaje de prueba (compra de licencias) cae en `ventas`.

```json
{
  "nodes": {
    "trigger": { "type": "trigger_webhook", "config": { "path": "/clasificar", "test_payload": { "message": "quiero comprar 100 licencias" } } },
    "router": {
      "type": "router",
      "config": {
        "mode": "llm_direct",
        "provider": "google",
        "model": "gemini-2.5-flash",
        "api_key": "${GEMINI_API_KEY}",
        "branches": [
          { "name": "ventas", "description": "El cliente quiere comprar, cotizar o contratar." },
          { "name": "soporte", "description": "El cliente tiene un problema o falla tecnica." },
          { "name": "facturacion", "description": "Dudas sobre pagos, facturas o cobros." }
        ]
      }
    },
    "responder_ventas": { "type": "llm_call", "config": { "provider": "google", "model": "gemini-2.5-flash", "api_key": "${GEMINI_API_KEY}", "system_message": "Sos del equipo de VENTAS. Responde amable y concreto a: {{message}}" } },
    "responder_soporte": { "type": "llm_call", "config": { "provider": "google", "model": "gemini-2.5-flash", "api_key": "${GEMINI_API_KEY}", "system_message": "Sos del equipo de SOPORTE tecnico. Responde amable y concreto a: {{message}}" } },
    "responder_facturacion": { "type": "llm_call", "config": { "provider": "google", "model": "gemini-2.5-flash", "api_key": "${GEMINI_API_KEY}", "system_message": "Sos del equipo de FACTURACION. Responde amable y concreto a: {{message}}" } },
    "out": { "type": "output", "config": {} }
  },
  "edges": [
    { "from": "trigger", "to": "router" },
    { "from": "router.ventas", "to": "responder_ventas" },
    { "from": "router.soporte", "to": "responder_soporte" },
    { "from": "router.facturacion", "to": "responder_facturacion" },
    { "from": "responder_ventas", "to": "out" },
    { "from": "responder_soporte", "to": "out" },
    { "from": "responder_facturacion", "to": "out" }
  ]
}
```

> Los nombres de rama deben ser slugs en minúsculas (`^[a-z][a-z0-9_]{0,63}$`).

Qué hace cada parte:
- `trigger`: recibe el mensaje de la persona en `message`.
- El edge **pelado** `trigger → router`: el payload del webhook entra al router por
  su puerto de entrada por defecto (`input`); la IA lo lee para clasificar.
- `router`: en `llm_direct` la IA lee las `description` de las ramas y elige una.
  El mensaje del ejemplo (compra) cae en `ventas`. Solo el puerto `router.ventas`
  emite payload; `router.soporte` y `router.facturacion` emiten `null`.
- Los edges **punteados** `router.ventas → responder_ventas`, etc.: cada rama
  dispara su especialista. Solo el especialista de la rama elegida corre.
- Cada `responder_* → out`: devuelve la respuesta final del especialista que disparó.

> **Variante modo B (`extract_and_route`).** Si querés ruteo auditable por reglas,
> poné `mode: "extract_and_route"`, agregá un `schema` con los campos a extraer, y
> en cada rama cambiá `description` por `when` (reglas declarativas como
> `{ "field": "intent", "equals": "sales" }`). El cableado por puertos
> (`router.<rama>`) es idéntico. Más estricto, pero menos flexible.

> **Leer la decisión.** Si un nodo posterior necesita saber por qué se eligió la
> rama, agregá un edge `router.__decision → <nodo>` y leelo con
> `{{__decision.selected_branch}}` / `{{__decision.reason}}` en su `config`.

---

## Para conectar todo

El detalle de cómo se nombran los puertos, cómo se escriben los `edges` y por qué,
y cómo entra el input al primer nodo está en [[building-graphs-core]]. Volvé ahí
cada vez que dudes del cableado.
