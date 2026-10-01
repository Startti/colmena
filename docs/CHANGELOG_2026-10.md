# Cambios recientes — 2026-10

> **Alcance:** Commits sobre `develop` desde el cierre de `2026-09`.

## Cómo leer este documento

Una sección por feature. Cada sección contiene:
- **Qué cambió** — efecto observable.
- **Documentación de referencia** — spec, plan, dev guide, schema.
- **Commits** — rango o lista.
- **Estado** — done / partial.

---

## 1. Tavily: el mensaje de la página de bloqueo ya no dice que la key está bien

**Qué cambia.** El texto fijo que §208 de [CHANGELOG_2026-09](CHANGELOG_2026-09.md) puso para un 403 con página
de bloqueo afirmaba que no era un problema de la key y que se levantaba en unos minutos. Medido el 2026-10-01, era
falso: la página salió 13+ horas seguidas y la causa era la key. El proveedor rechazaba con la página esa key
concreta cuando llegaba desde IPs de Google Cloud (la misma key desde otra red daba 200; sin key desde la misma red,
el 401 JSON normal; otra key desde Google Cloud, 200); cambiarla lo arregló. Los agentes leyeron «not a problem with
the API key / clears within a few minutes», le dijeron al usuario que esperara, y un chat reintentó tres veces y
después contestó de memoria. El mensaje nuevo:

> The search provider refused this search with a block page (HTTP 403); the cause may be the network the call
> comes from or the provider blocking this API key. Retrying in this turn will not fix it: tell the person web
> search is unavailable and do not answer as if you had searched.

La clasificación no cambia: sigue siendo `Upstream { status: 403 }`, recuperable, el use case no lo reintenta (solo
5xx y transporte) y llega al modelo como `upstream_error` con `retryable: false`. Guía:
[25_web_nodes.md](developer_guide/25_web_nodes.md).
**Tests.** En `tavily_adapter`: el mensaje nombra las dos causas (la red y la key bloqueada), pide no reintentar,
decir que la búsqueda no está disponible y no contestar como si se hubiera buscado, y es ASCII; un test nuevo fija que
no dice que la key está bien ni promete que esperar lo arregla; `search` sobre un servidor que contesta 403 con la
página ya no trae «not a problem with the API key». En `search_use_case`: un `Upstream` 403 se intenta una sola vez.
En `tavily_client`: un `Upstream` 403 llega al modelo con `retryable: false` y el mensaje tal cual.
**ADP.** Subir el pin; nada más. **Estado.** done.
