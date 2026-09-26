# Un destino que viene de datos se marca solo en direcciones públicas

**Acción de ADP:** ninguna de código. Una tool con `base_url` abierto que deba llegar a un host no
público lo lista en `allowed_hosts` del autor. En desarrollo local, arrancar el worker con
`COLMENA_ATTACHMENT_ALLOW_PRIVATE_HOSTS=1` si una tool elige `localhost`; nunca en un worker
compartido.

## Qué cambia

- **`http_request`** (CHANGELOG 2026-09 §143): si `base_url` viene de datos (un edge que lo nombra, un
  campo abierto de la tool, una tool que llega como dato) y no es el origen del `base_url` del
  autor, el nodo marca solo direcciones públicas, también en cada redirect, y sin proxy. Un host en
  `allowed_hosts` se marca en cualquier dirección. El destino que fija el autor no cambia.
- **`socketio_request`** (§125): una `url` que viene de datos, fuera de `allowed_hosts`, conecta solo a
  direcciones públicas y solo por `transport: "websocket"`; `any` y `polling` se niegan.

## Qué ve ADP

- Nada en el SSE. Un destino rechazado es un error del nodo (o de la tool), sin la URL:
  `http_request: a destination that comes from data connects only to a public address …` o
  `socketio_request: a url that comes from data connects …`.
- Superficie de Rust: `SocketIoNode` deja de ser un struct unitario; se construye con
  `SocketIoNode::default()`.

## Qué se rompe si se ignora

Nada en producción mientras las tools con destino abierto apunten a hosts públicos. Una que apunte
a un servicio interno sin listarlo en `allowed_hosts` deja de conectarse.
