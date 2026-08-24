# El frame de frontera de un nodo anidado ahora lleva `provider` y `model`

**Acción de ADP: ninguna obligatoria.** Es puramente aditivo. Dos campos que
llegaban en `null` ahora traen valor, y un objeto que llegaba vacío ahora trae
dos claves.

## Qué pasaba

Un `llm_call` (o un `for_each`) despachado como tool emitía su frame de
frontera `subgraph-node-start` con `config` e `inputs` vacíos:

```jsonc
{
  "type": "subgraph-node-start",
  "node_id": "consultar_hijo",
  "node_type": "llm_call",
  "config": {},          // ← vacío
  "inputs": {},
  "level": 1,
  "path": "padre>consultar_hijo"
}
```

El motor puebla su tabla de metadatos de nodo leyendo `provider` y `model` de
ese frame, así que la fila de ese nodo en `usage-summary` salía así:

```jsonc
{
  "node_id": "consultar_hijo",
  "model": null,          // ←
  "provider": null,       // ←
  "prompt_tokens": 415,
  "cache_read_tokens": 3546,
  "cache_write_tokens": 0,
  "total_tokens": 4029
}
```

Los tokens del nodo anidado se **atribuían** bien, pero **no se podían
tarifar**: no había forma de saber qué modelo los quemó. Y el `fixed_config` de
un tool es libre de nombrar un provider y un modelo distintos a los del agente
que lo despacha — el patrón del hijo en tier barato — así que tomar los valores
del padre no era un sustituto válido.

## Qué cambia

El frame de frontera lleva la identidad del nodo que va a correr:

```jsonc
{
  "type": "subgraph-node-start",
  "node_id": "consultar_hijo",
  "node_type": "llm_call",
  "config": { "provider": "google", "model": "gemini-2.5-flash" },
  "inputs": {},
  "level": 1,
  "path": "padre>consultar_hijo"
}
```

Y la fila del `usage-summary` queda tarifable: los mismos campos de token que
antes, pero con `"provider": "google"` y `"model": "gemini-2.5-flash"` en vez de
`null`.

Verificado en vivo con un padre Anthropic (`claude-sonnet-4-6`) y un hijo Gemini
(`gemini-2.5-flash`) en el mismo run: cada fila reporta **lo suyo**, el hijo no
hereda del padre.

## Es una allowlist, y tiene que seguir siéndolo

`config` en este frame lleva **solo** `provider` y `model`. No es una decisión
estética: en el punto donde se emite la frontera, los inputs del nodo ya tienen
el `fixed_config` mezclado y los secure values **descifrados** — el `api_key`
resuelto entre ellos — y el mapper de SSE solo limpia claves con prefijo `__` y
`session_id`. Volcar los inputs enteros ahí pondría una credencial viva en el
stream. Por eso el frame salía vacío originalmente.

Hay un test (`never_carries_anything_outside_the_allowlist`) que falla si
alguien amplía la lista con algo que pueda contener un secreto.

## Qué gana ADP si lo usa

Poder calcular costo por nodo anidado, que hasta ahora era imposible: las
tarifas son por modelo, y el modelo del nodo anidado no viajaba. Si el cálculo
de costo agrupa por `node_id` sin mirar `model`, ahora puede mirarlo.

## Qué se rompe si se ignora

Nada. Un consumidor que hoy trata `model`/`provider` como opcionales sigue
funcionando igual.
