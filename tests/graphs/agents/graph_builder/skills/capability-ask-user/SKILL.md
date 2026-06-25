---
name: capability-ask-user
description: Use when the built graph needs to pause and ask the end-user for a value or a decision before continuing (human-in-the-loop). Covers the suspend node.
---

# Pedirle algo a la persona usuaria (Human-in-the-Loop)

A veces el flujo no puede seguir solo: necesita que la persona confirme algo,
elija una opción o escriba un dato antes de continuar. Para eso existe el nodo
**`suspend`**. Cuando el flujo llega a un `suspend`, **se pausa**, le muestra una
pregunta a la persona, y **espera la respuesta**. Apenas la persona responde, el
flujo continúa al siguiente nodo llevándose esa respuesta.

Es el mecanismo canónico de Colmena para flujos *Human-in-the-Loop* (HITL):
aprobaciones, confirmaciones, recolección de un dato libre, elegir entre
opciones, etc.

> Si lo que necesitás pedir es un **secreto** (una API key, una contraseña), eso
> NO va por `suspend` sino por otro nodo distinto (`secure_suspend`). Esta skill
> cubre solo `suspend` — preguntas y decisiones normales, no secretos.

---

## 1. El nodo `suspend` — configuración

| Campo | ¿Obligatorio? | Qué es |
|---|---|---|
| `id` | **Sí** | Identificador estable de la pregunta. Solo letras, números, `_` y `-` (charset `[A-Za-z0-9_-]`), entre **1 y 64 caracteres**. Sin espacios ni acentos. Es el id con el que se "amarra" la respuesta de la persona — no cambia aunque renombres el nodo. |
| `question` | No (default `"What is your input?"`) | La pregunta que ve la persona. Escribila clara y específica: que se entienda qué necesitás para seguir. |
| `question_type` | No (default `"open"`) | `"open"` = texto libre. `"choice"` = se muestran opciones para elegir. |
| `options` | No (default `null`) | Lista de opciones sugeridas. **Solo aplica si `question_type` es `"choice"`.** |

Estos son los nombres exactos de los campos. No inventes otros.

> **`options` es solo una sugerencia visual, NO una lista cerrada.** Aunque
> definas `options: ["aprobar", "rechazar"]`, la persona puede responder con
> cualquier texto libre — el sistema acepta cualquier respuesta, esté o no en la
> lista. `options` solo le da pistas rápidas a la interfaz; no fuerza una
> respuesta concreta.

### Ejemplo mínimo

```json
{
  "approval": {
    "type": "suspend",
    "config": {
      "id": "approval",
      "question": "¿Aprobás continuar?"
    }
  }
}
```

### Ejemplo con opciones

```json
{
  "pick_env": {
    "type": "suspend",
    "config": {
      "id": "pick_env",
      "question": "¿A qué entorno hacemos deploy?",
      "question_type": "choice",
      "options": ["staging", "production", "rollback"]
    }
  }
}
```

---

## 2. Entradas y salidas (cómo conectarlo)

- **Entrada por defecto:** `question` — opcionalmente podés conectarle una
  pregunta dinámica desde un nodo anterior; si llega por conexión, gana sobre la
  `question` del config.
- **Salida por defecto:** `answer_received` — lleva el **texto crudo que
  respondió la persona** al siguiente nodo.

Por eso, una conexión simple `{ "from": "<nodo_suspend>", "to": "<siguiente>" }`
hace que la respuesta de la persona fluya directamente al nodo de abajo.

---

## 3. Ejemplo completo y ejecutable (VERBATIM)

Pregunta tipo `choice` que continúa a otro nodo. Copiá este grafo tal cual:

```json
{
  "nodes": {
    "start": {
      "type": "input",
      "config": {
        "msg": "Preparando el deploy..."
      }
    },
    "pick_env": {
      "type": "suspend",
      "config": {
        "id": "pick_env",
        "question": "¿A qué entorno hacemos deploy?",
        "question_type": "choice",
        "options": ["staging", "production", "rollback"]
      }
    },
    "final": {
      "type": "log"
    }
  },
  "edges": [
    {
      "from": "start",
      "to": "pick_env"
    },
    {
      "from": "pick_env",
      "to": "final"
    }
  ]
}
```

Qué pasa al ejecutarlo:

1. El flujo llega a `pick_env` y **se pausa**, mostrando la pregunta y las tres
   opciones sugeridas.
2. La persona responde (cualquier texto — por ejemplo `staging`, o incluso algo
   fuera de la lista, porque `options` no obliga).
3. El flujo **continúa** a `final`, que recibe la respuesta por la salida
   `answer_received`.

Para reanudar, la respuesta se entrega amarrada al `id` del nodo
(`pick_env` en este caso), en formato `Q[pick_env]: ...` / `A[pick_env]: ...`.
El `id` del payload de respuesta debe coincidir **exactamente** con el `config.id`
del nodo.

---

## 4. Buenas prácticas

- Dale a cada `suspend` un `id` corto, descriptivo y sin espacios
  (`approval`, `pick_env`, `confirm_transfer`). Recordá: 1–64 caracteres,
  solo letras/números/`_`/`-`.
- Escribí la `question` como se la mostrarías a una persona real: clara y con
  contexto suficiente para que sepa qué decidir.
- Usá `question_type: "choice"` + `options` cuando quieras guiar la decisión,
  pero recordá que la persona igual puede escribir libremente.
- Conectá la salida del `suspend` al nodo que debe usar la respuesta — el texto
  llega por `answer_received`.

---

Para cómo armar y conectar los nodos del grafo en general, ver
[[building-graphs-core]].
