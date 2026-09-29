# Pre-flight: una respuesta transitoria no es una key rechazada

**Acción de ADP:** subir el pin. Nada más: ADP no hace `match` exhaustivo sobre `LlmError`
(en ADP, `git grep -n 'LlmError' apps/service` no da nada).

## Qué cambia

- `LlmError::ProviderUnavailable { status }` es nuevo: un 408, 429, 500, 502, 503, 504 o 529 de un proveedor.
  `LlmError::is_transient()` dice si un error no dice nada de la key (esa variante,
  `RateLimitExceeded` o `NetworkError`).
- «Pre-flight: provider X rejected the API key» sale solo con un veredicto: 401/403 o cualquier otro
  estado fuera de esa lista (un 4xx, o un 5xx como el 501). Solo los veredictos quedan en la caché de pre-flight.
- Sin respuesta tras tres intentos (backoff exponencial con jitter), la corrida arranca igual, el
  error llega del nodo y queda un `warn` de `colmena::preflight`.
- Los errores de red de Gemini con la key en el query (validación, TTS, Files API) no citan la URL.

## Qué ve ADP

Antes, un 503 pasajero del endpoint de modelos de un proveedor cortaba toda corrida con esa key
en ese proceso durante el TTL de la caché (60 s por defecto) con «rejected the API key», y un
agente que volvía a llamar enseguida a un sub-agente recibía el mismo error desde la caché.
Después, esa corrida sigue.

## Qué se rompe si se ignora

Nada al compilar. Sin subir el pin, el corte de un minuto sigue.
