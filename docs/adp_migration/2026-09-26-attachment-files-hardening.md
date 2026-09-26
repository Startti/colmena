# Adjuntos: el hijo no lee `files` de su estado, `path` solo en local y URLs solo públicas

**Acción de ADP:** ninguna de código. En desarrollo local, arrancar el worker con
`COLMENA_ATTACHMENT_ALLOW_PRIVATE_HOSTS=1` si sirve adjuntos desde `localhost`. En un worker
compartido no fijar esa variable ni `COLMENA_LOCAL=true`.

## Qué cambia

- **El `llm_call` de un hijo ya no lee `files` de su estado** (CHANGELOG 2026-09 §138). `files` es un
  campo del autor de `llm_call`: lo fijan `config.files` o un edge que nombra el campo
  (`to: "<nodo>.files"`), o un parámetro `files` que la tool ofrece. `subgraph` sigue copiando
  `files` al estado del hijo, pero su `llm_call` no lo toma de ahí; tampoco del estado global, de
  una fila de `for_each` ni de un argumento que la tool no ofrece.
- **`files[].path` se lee solo con `COLMENA_LOCAL=true`** (§138). Fuera de ese modo la entrada
  falla con `PathFieldNotAllowed`, sin leer nada.
- **Toda URL de adjunto se baja con un cliente guardado** (§137): solo `http`/`https`, solo
  direcciones públicas, tope `COLMENA_ATTACHMENT_MAX_BYTES` (100 MiB por defecto).

## Qué ve ADP

- Nada en el SSE. ADP pone los adjuntos en `config.files` de los `llm_call` de primer nivel, con
  URLs firmadas de GCS (públicas), y nunca usa `path` ni pasa `files` a un sub-agente.
- Superficie de Rust: se quita `SignedUrlDownloader::with_client`; `LlmError` suma
  `AttachmentUrlRefused` y `AttachmentTooLarge` (§137) y `PathFieldNotAllowed` (§138). Un `match`
  exhaustivo sobre `LlmError` necesita los brazos nuevos.
- Desarrollo local: un adjunto en `localhost` (o en otra dirección no pública) falla con
  `attachment URL refused: destination is not a public address` si el worker no arrancó con
  `COLMENA_ATTACHMENT_ALLOW_PRIVATE_HOSTS=1` (o `true`).

## Qué se rompe si se ignora

Nada en producción. Un grafo cuyo hijo esperara los adjuntos del padre en su estado deja de
verlos: se le pasan por `config.files` del hijo o por un parámetro `files` que la tool ofrezca.
Reemplaza el punto «`files` sigue pasando» de
[la nota del 2026-08-25](2026-08-25-subgraph-plumbing-child-state.md).
