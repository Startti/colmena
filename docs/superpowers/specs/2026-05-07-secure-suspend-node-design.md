# Diseño: nodo `secure_suspend`

**Estado:** propuesta
**Fecha:** 2026-05-07
**Autor:** Daniel García (Startti)

## Contexto

El meta-agente "canvas builder" (ver `tests/graphs/external/socketio_canvas_builder.json`) construye grafos de ADP para el usuario. Cuando un grafo nuevo necesita autenticarse contra una API externa, el meta-agente debe colocar credenciales del usuario en algún campo de los nodos que crea. Las formas de auth varían:

- **Token estático en header.** El más simple — un bearer token o `X-API-Key` en `headers` o `bearerToken` del `apiCall` (caso típico HubSpot Private App, Stripe).
- **Par de credenciales en body para intercambio OAuth.** El usuario provee `client_id` + `client_secret`, el grafo los manda en el body form-urlencoded de un POST `/oauth2/token` (`secure: true`), recibe un access token de corta vida, y lo enchufa a las tools downstream vía `${context.X}` — patrón implementado en `tests/graphs/advanced/trip_assistant.json` para Amadeus.
- **Credenciales en query string.** Algunas APIs viejas piden `?api_key=...`.
- **Connection string en `databaseQuery`.** Password embebido en una URL Postgres.

En todos los casos el flujo de **recolección** del valor desde el usuario es el mismo, y en todos el flujo de **inyección** al ejecutar el nodo no-LLM ya está resuelto por `SecureValueService::inject_secrets`. Lo que falta es el puente entre ambos: un nodo que el meta-agente pueda invocar como tool LLM para preguntar al usuario y obtener un handle.

Hoy las dos únicas opciones son:

1. Que el meta-agente use `${ENV_VAR}` y delegar al usuario definir esa variable fuera de banda. No es operable: el meta-agente no puede recolectar el secreto en la conversación.
2. Que el meta-agente le pida el secreto al usuario por chat. El valor pasa por el contexto del LLM y por logs. Inaceptable.

Lo que sí tenemos en Colmena: `SecureValueService` (`src/libs/colmena/src/dag_engine/application/secure_value_service.rs`) ya guarda valores cifrados y los inyecta automáticamente cuando un nodo no-LLM recibe un input con un placeholder de la forma `<...>`. Ese mecanismo lo usa hoy `http_request` con `secure: true` para hashear respuestas.

Lo que falta es **un nodo que combine `suspend` con escritura a la tabla de secure values**, de modo que el LLM pueda invocarlo como tool, recolectar un secreto del usuario, y obtener de vuelta solo un handle — nunca el valor real.

## Objetivo

Agregar un nuevo `ExecutableNode` llamado `secure_suspend` que:

1. Pause el DAG con una pregunta al usuario (mismo contrato que `suspend` actual).
2. Al recibir la respuesta, persista el valor en la tabla de secure values cifrado, indexado por `session_id`.
3. Devuelva al output **solo** el handle (`<sv_<name>>`), nunca el valor en claro.
4. Sea invocable desde `tool_configurations` de un `llm_call` para que el meta-agente pueda llamarlo como tool y obtener handles que luego pega en los nodos del canvas que crea.

## Arquitectura

### Dónde vive

Un archivo nuevo: `src/libs/colmena/src/dag_engine/infrastructure/nodes/secure_suspend.rs`. Registrado en `src/libs/colmena/src/dag_engine/infrastructure/registry.rs` con el `node_type: "secure_suspend"`.

### Qué reutiliza

- **Mecánica de pausa/resume**: idéntica a `SuspendNode`. Detecta `__colmena_resume_answer` en inputs para distinguir suspend-path de resume-path.
- **Persistencia cifrada**: el repo `SecureValueRepository` (impl `PostgresSecureValueRepository`) ya tiene `persist(session_id, source_node_id, hash_key, real_value, field_name)` y `decrypt(...)`. El nodo nuevo invoca `persist` en el resume-path.
- **Inyección automática**: `SecureValueService::inject_secrets` ya recorre los inputs antes de ejecutar nodos no-LLM y reemplaza cualquier string que matcheé `<...>`. No necesita cambios. Nuestros handles `<sv_<name>>` van a ser inyectados gratis cuando aparezcan en un `http_request` o cualquier otro nodo no-LLM downstream.

### Qué NO toca

- `LlmCallNode` y su pipeline de tool-execution: el nodo se invoca a través del mismo path que cualquier otro tool registrado. No hay cambios al motor.
- `SuspendNode` actual: se mantiene tal cual. Ortogonal.
- `SecureValueService`: no se modifica. Solo lo consume el nodo nuevo.

### Cómo accede al repo

El service container (`src/libs/colmena/src/shared/service_container.rs`) ya inyecta dependencias a los nodos. El nuevo nodo recibirá un `Arc<dyn SecureValueRepository>` por construcción (igual que `LlmCallNode` recibe sus dependencias). Detalles de wiring se dejan al plan de implementación; lo único que el spec compromete es: **el repo se inyecta, no se construye dentro del nodo**.

## API del nodo

El nodo recolecta **uno o más secretos en una sola pausa**. El meta-agente le pasa una lista de pares `{question, name}`, el usuario teclea todos los valores en un único turno, y el nodo retorna un mapa de handles.

### Config schema

```jsonc
{
  "secrets": [
    { "question": "string, required", "name": "string, required" },
    ...
  ],
  "id": "string, optional"  // ID estable del bloque de preguntas (default: __node_id)
}
```

Decisiones:

- **`secrets` es una lista**, mínimo 1 ítem, máximo razonable 8. El nodo acepta también una sola entrada — un secreto suelto es solo un `secrets: [{...}]` de longitud 1.
- **`name` por ítem es obligatorio** y se usa como sufijo del handle: cada ítem genera un handle `<sv_<name>>`. Los names dentro de una misma llamada deben ser únicos (validado en suspend-path antes de pausar).
- **Cada `name` debe matchear `^[a-z][a-z0-9_]{2,63}$`**. Reduce ambigüedad de inyección y evita handles raros.
- **No hay `question_type`** ni `options`. Un secreto siempre es texto libre.

### Salida del nodo

**Suspend-path** (primera ejecución, sin `__colmena_resume_answer`):

```json
{
  "__colmena_status": "SUSPENDED",
  "questions": [
    { "id": "<id>__1", "question": "<pregunta_1>", "type": "secret", "options": null },
    { "id": "<id>__2", "question": "<pregunta_2>", "type": "secret", "options": null }
  ]
}
```

- `type: "secret"` permite a las UIs (ADP frontend) renderizar un input enmascarado / sin autocompletar. Si la UI no lo distingue, fallback a `text` es seguro porque el valor nunca se logea.
- A diferencia del `SuspendNode` actual, **no se emite el campo `question` legacy** (top-level) — con múltiples preguntas dejaría de tener sentido. La canónica `questions[]` ya está soportada por las UIs.

**Resume-path** (con `__colmena_resume_answer`):

```json
{
  "status": "resumed",
  "handles": {
    "amadeus_client_id":     "<sv_amadeus_client_id>",
    "amadeus_client_secret": "<sv_amadeus_client_secret>"
  }
}
```

**Crítico**: el output **nunca** contiene los valores reales — solo los handles, indexados por el `name` del meta-agente. Si la lista era de un solo ítem, `handles` igual es un mapa con una clave (consistencia, así el meta-agente siempre lee `tool_result.handles.<name>`).

### Protocolo de respuesta del UI / cliente

`__colmena_resume_answer` es **un único string** con el siguiente formato canónico:

```
<pregunta_1>
<valor_1>
<pregunta_2>
<valor_2>
```

Es decir: cada pregunta literal del array `questions[]` aparece tal cual emitida, seguida de un newline, seguida del valor que el usuario tecleó, seguida de un newline, y luego la siguiente pregunta. El UI debe reproducir el texto exacto de cada pregunta como anclas de parsing.

**Algoritmo de parsing en el nodo:**

1. Tomar `secrets` del config en orden. Para cada `secrets[i].question`, buscarlo en el string de respuesta. Las preguntas deben aparecer en el mismo orden.
2. Para cada pregunta encontrada en posición `p_i`, el valor asociado es la subcadena entre el `\n` que sigue a la pregunta `i` y el inicio de la pregunta `i+1` (o el final del string para la última).
3. Trimmear solo trailing newlines del valor — espacios internos y newlines internos se preservan tal cual (importante para RSA private keys multilínea).

Esto significa que el UI puede simplemente concatenar `pregunta\nvalor\n` por cada par sin preocuparse de escape, porque el ancla es el texto exacto de la pregunta.

**Casos límite:**

- Una pregunta del config no aparece en el answer string → error `missing answer for secret '<name>' (question not found)`.
- El valor entre dos preguntas es vacío → error `empty value for secret '<name>'`.
- Dos preguntas del config tienen el mismo texto → error en suspend-path validation antes de pausar (`duplicate question text — make each question unique`).
- El answer string contiene una pregunta más de las pedidas → ignorada (el meta-agente solo pidió N).

### Errores

El motor envuelve cualquier error retornado por `execute` en `DagError::NodeExecution(String)`. El nodo retorna mensajes específicos:

| Caso | Path | Mensaje |
|---|---|---|
| `secrets` ausente o vacío | suspend | `secure_suspend: secrets list missing or empty` |
| Algún `name` inválido | suspend | `secure_suspend: name '<x>' invalid (expected lowercase slug, 3-64 chars)` |
| Names duplicados dentro del call | suspend | `secure_suspend: duplicate name '<x>' in secrets list` |
| Preguntas duplicadas dentro del call | suspend | `secure_suspend: duplicate question text — make each question unique` |
| Colisión de handle (ya existe en sesión) | resume | `secure_suspend: handle <sv_<name>> already exists in session — use a different name` |
| Pregunta no encontrada en answer | resume | `secure_suspend: missing answer for secret '<name>' (question not found in response)` |
| Valor vacío | resume | `secure_suspend: empty value for secret '<name>'` |
| Persist falla | resume | propagado del repo |

Validaciones de suspend-path corren **antes** de emitir SUSPENDED: si fallan, el grafo no se pausa, lo cual evita ciclos suspend-fail-suspend que confunden al usuario.

La detección de colisión requiere una operación adicional en `SecureValueRepository`: un `exists(session_id, hash_key) -> bool`, o cambiar `persist` a fallar al ya existir. El plan de implementación elige el camino menos invasivo.

## Uso como tool LLM

El nodo se expone en `tool_configurations` así:

```jsonc
{
  "ask_secret": {
    "name": "ask_secret",
    "node_type": "secure_suspend",
    "description": "Ask the user for ONE OR MORE secrets in a SINGLE prompt cycle. Use this whenever a workflow you are building needs credentials (API token, OAuth client_id+secret pair, AWS access keys, DB password, etc.). Pass an array of {question, name} pairs — ALL the secrets needed for the same external service in one call. The user's answers are stored encrypted; you receive a HANDLES MAP `{ <name>: <sv_<name>> }`. Paste each handle into ANY field of any non-LLM node — `bearerToken`, an entry inside `headers`, a value inside an OBJECT-form `body`, a `query_params` value, a Postgres connection string, etc. The handle is replaced by the real value at execution time, never by you. NEVER ask for secrets via plain chat messages. Compose each question as a short, specific request: name the service and the credential type. Good: 'Pega el HubSpot private app token (empieza con \"pat-\").', 'Cuál es tu Amadeus client_id?'. Bad: 'dame tu token'. IMPORTANT placement rule: the handle must appear as a COMPLETE string value, never embedded inside a longer string. Object body form works: `{ \"client_id\": \"<sv_xxx>\", \"client_secret\": \"<sv_yyy>\" }`. Concatenated string form does NOT: `\"client_id=<sv_xxx>&client_secret=<sv_yyy>\"`. Each question must be UNIQUE within a call (the response anchors on question text).",
    "node_schema": {
      "secrets": {
        "type": "array",
        "required": true,
        "description": "List of secrets to ask for in this single prompt cycle. Min 1, max 8. Each item has a question and a name slug.",
        "items": {
          "type": "object",
          "properties": {
            "question": {
              "type": "string",
              "required": true,
              "description": "The exact text shown to the user. Mention the service and credential type. Do NOT include example values. Must be unique within this call."
            },
            "name": {
              "type": "string",
              "required": true,
              "description": "Slug identifying this secret. Lowercase letters/digits/underscore, 3-64 chars. Examples: 'hubspot_private_token', 'amadeus_client_id', 'amadeus_client_secret'. Used as the suffix of the returned handle <sv_<name>>."
            }
          }
        }
      }
    }
  }
}
```

Notas importantes:

- La `description` arriba es **prescriptiva**: dice cuándo llamar el tool y cómo redactar la pregunta. Esto es lo que el usuario explícitamente pidió: "cuando se agregue como una tool debe tener una descripción específica de cuándo llamarlo y cómo debe componer las preguntas".
- `node_schema` solo expone `question` y `name`. No hay forma de que el LLM pase un valor de secreto.
- Esta plantilla se incluye en el spec del par de grafos canvas-builder (Spec 2) y en la skill `adp-node-catalog` (Spec 3).

## Patrones de uso comunes

### Patrón 1 — Token estático en header (HubSpot, Stripe simple)

Una sola llamada a `ask_secret` con un solo ítem en `secrets`. El handle va a `bearerToken` o a una entrada de `headers`.

```jsonc
ask_secret({
  secrets: [{ question: "Pega tu HubSpot private app token", name: "hubspot_private_token" }]
})
// → { handles: { hubspot_private_token: "<sv_hubspot_private_token>" } }

// El meta-agente luego usa:
"bearerToken": "<sv_hubspot_private_token>"
// o
"headers": { "X-API-Key": "<sv_hubspot_private_token>" }
```

### Patrón 2 — Intercambio OAuth con par de credenciales (Amadeus)

**Una sola llamada** a `ask_secret` con DOS ítems. El usuario teclea ambos valores en un solo turno. Luego el meta-agente construye dos nodos:

1. Un `apiCall` que POSTea a `/oauth2/token` con el par de credenciales en body **forma objeto** (no string urlencoded), `secure: true`. El output `access_token` se hashea automáticamente como `<value_N>` por la lógica existente de `SecureValueService::hash_output`.
2. Un segundo `apiCall` para la API de negocio (`/v2/shopping/flight-offers`, etc.) cuyo `bearerToken` recibe el `access_token` por edge desde el primer nodo, con inyección automática.

```jsonc
// Llamada del meta-agente al tool:
ask_secret({
  secrets: [
    { question: "Cuál es tu Amadeus client_id?",     name: "amadeus_client_id" },
    { question: "Cuál es tu Amadeus client_secret?", name: "amadeus_client_secret" }
  ]
})
// → { handles: {
//       amadeus_client_id:     "<sv_amadeus_client_id>",
//       amadeus_client_secret: "<sv_amadeus_client_secret>"
//   } }

// Nodo OAuth construido por el meta-agente:
{
  "url": "https://api.amadeus.com",
  "endpoint": "/v1/security/oauth2/token",
  "method": "POST",
  "secure": true,
  "headers": { "Content-Type": "application/x-www-form-urlencoded" },
  "body": {
    "grant_type": "client_credentials",
    "client_id": "<sv_amadeus_client_id>",
    "client_secret": "<sv_amadeus_client_secret>"
  }
}
```

Pre-condición: el nodo `http_request` debe serializar un body de forma `application/x-www-form-urlencoded` cuando el `Content-Type` lo indica y el body es objeto. Esto se valida en el plan de implementación; si no lo hace hoy es un bug separado del nodo HTTP, no de `secure_suspend`.

### Patrón 3 — Connection string de DB

Una llamada a `ask_secret` con un ítem `name: "main_db_url"`, el handle va al campo `connection_url` de un `databaseQuery`. Mismo mecanismo.

## Flujo end-to-end

Ejemplo con el caso Amadeus (dos secretos en una pausa):

```
[meta-agente, llm_call]
   │ piensa: "necesito client_id + client_secret de Amadeus"
   │ tool_call: ask_secret({ secrets: [
   │     { question: "Cuál es tu Amadeus client_id?",     name: "amadeus_client_id" },
   │     { question: "Cuál es tu Amadeus client_secret?", name: "amadeus_client_secret" }
   │ ]})
   ▼
[secure_suspend node, suspend-path]
   │ valida names (regex, únicos) y preguntas (únicas) → OK
   │ emite __colmena_status: SUSPENDED + questions:[
   │   {id:"...__1", question:"Cuál es tu Amadeus client_id?",     type:"secret"},
   │   {id:"...__2", question:"Cuál es tu Amadeus client_secret?", type:"secret"}
   │ ]
   ▼
[engine] congela DAG, persiste session, retorna a cliente
   ▼
[ADP frontend] renderiza dos inputs enmascarados (uno por pregunta)
   ▼
[usuario] teclea ambos valores
   ▼
[ADP] POST /chat/run con sessionId + prompt =
   "Cuál es tu Amadeus client_id?
   AMG-CLI-ID-abc
   Cuál es tu Amadeus client_secret?
   AMG-CLI-SEC-xyz"
   ▼
[engine] reanuda DAG con __colmena_resume_answer = el string anterior
   ▼
[secure_suspend node, resume-path]
   │ parsea el string: ancla en cada question del config para extraer su valor
   │ valida no-colisión de "<sv_amadeus_client_id>" y "<sv_amadeus_client_secret>"
   │ repo.persist por cada uno
   │ retorna { status: "resumed", handles: {
   │     amadeus_client_id:     "<sv_amadeus_client_id>",
   │     amadeus_client_secret: "<sv_amadeus_client_secret>"
   │ }}
   ▼
[meta-agente, llm_call] continúa
   │ tool_result visible al LLM = solo handles, los valores reales nunca aparecen
   │ tool_call: create_canvas_node(node={ type: "apiCall", data: { config: {
   │     url: "https://api.amadeus.com",
   │     endpoint: "/v1/security/oauth2/token",
   │     method: "POST",
   │     secure: true,
   │     headers: { "Content-Type": "application/x-www-form-urlencoded" },
   │     body: {
   │       grant_type: "client_credentials",
   │       client_id:     "<sv_amadeus_client_id>",
   │       client_secret: "<sv_amadeus_client_secret>"
   │     }
   │ }}})
   ▼
[ADP runtime, en una ejecución posterior del agente creado]
   │ engine va a ejecutar el apiCall (un http_request)
   │ inject_secrets recorre inputs, encuentra los dos handles, repo.decrypt cada uno
   │ reemplaza por los valores reales JUSTO antes del request HTTP
   ▼
[http_request] dispara el POST con el body real, el LLM nunca vio los valores
```

Pre-condición de la última fase: ADP y Colmena comparten la misma `DATABASE_URL` y por tanto la misma tabla de secure values. Confirmado en el brainstorm. La inyección funciona siempre que la `session_id` con la que el agente creado ejecute coincida con la que persistió el secreto. **Ojo**: si el secreto se persiste con `session_id = X` y el agente creado se ejecuta más tarde con `session_id = Y`, el handle no resuelve. Ver "Modos de falla" abajo.

## Modos de falla y consideraciones de seguridad

### Scope de sesión del secreto

`PostgresSecureValueRepository::persist` indexa por `session_id`. Eso significa:

- Un secreto persistido en la sesión del **meta-agente** (la conversación de construcción) NO es visible en una sesión separada del **agente construido**.
- Para que el handle resuelva en producción, el agente construido debe ejecutarse en la misma `session_id`, o necesitamos una promoción explícita del secreto a un scope más amplio.

**Decisión para esta versión**: el spec asume que el flujo de prueba (`/chat/run` con `origin=AGENT_TEST`) reusa la `session_id` del meta-agente o que el meta-agente pasa explícitamente `sessionId` al invocar `test_agent`. Para uso productivo (otro usuario ejecuta el agente nuevo días después), se requiere una promoción a un scope distinto (workspace, environment, group). Eso queda **fuera del alcance** de este spec — se trata como pre-requisito a resolver en un spec posterior si las pruebas iniciales lo confirman.

### Logs

`SuspendNode` actual logea la `question` pero no el `answer_received`. El nuevo nodo no debe logear:

- El valor real bajo ninguna circunstancia.
- El handle puede logearse sin riesgo (es opaco).

Test específico: ejecutar el nodo en resume-path con un valor predecible y verificar que no aparece en el `tracing` capturado.

### Observabilidad

El `ExecutionObserver` recibe el output del nodo. Nuestro output en resume-path solo contiene `{status, handle}`, así que cualquier observer (incluyendo el que ADP usa para mostrar el `toolCalls[]` array en `/chat/run`) ve solo el handle. Esto preserva la promesa.

### Colisión vs idempotencia

Decidí "colisión = error" en lugar de "colisión = sobrescribir" o "colisión = idempotencia silenciosa" porque:

- Sobrescribir silenciosamente puede llevar a que un agente quede usando un secreto distinto del que el usuario tecleó hace 30 segundos. Bug pesadillesco.
- Idempotencia silenciosa (el segundo call con el mismo `name` retorna el handle existente sin pedir al usuario) es ergonómica pero implica que el meta-agente puede reusar secretos sin saber que ya existían — sorpresivo.
- Error explícito fuerza al meta-agente a manejar el caso conscientemente (preguntar al usuario "ya tenía un token guardado, ¿lo reuso?", o usar un `name` versionado).

Si en uso real esto resulta molesto, se relaja en una iteración futura. Easier to loosen than tighten.

## Plan de testing

### Unit tests (en el archivo del nodo)

1. `suspend_path_emits_n_questions_with_secret_type` — secrets de longitud 2 produce dos entradas en `questions[]`, ambas con `type:"secret"`, status SUSPENDED.
2. `suspend_path_validates_name_format` — `name` con espacios/mayúsculas falla antes de pausar.
3. `suspend_path_rejects_duplicate_names` — dos ítems con el mismo `name` fallan.
4. `suspend_path_rejects_duplicate_questions` — dos ítems con el mismo `question` fallan (rompería el parser).
5. `suspend_path_rejects_empty_secrets_list` — `secrets: []` falla.
6. `resume_parses_two_secrets_correctly` — con un mock repo y answer `"q1\nval1\nq2\nval2"`, verifica que `persist` se invoca dos veces con los pares correctos y output es `{status:"resumed", handles:{n1:"<sv_n1>", n2:"<sv_n2>"}}`. **Crítico**: assert que el output NO contiene `val1` ni `val2` como strings.
7. `resume_preserves_internal_newlines_in_value` — con un valor multilínea (RSA private key), el parser preserva los newlines internos porque ancla en el texto literal de la siguiente pregunta.
8. `resume_errors_on_missing_question_in_answer` — answer string que no contiene una de las preguntas del config emite el error específico.
9. `resume_errors_on_empty_value` — pregunta seguida inmediatamente por la siguiente pregunta (sin valor) emite el error específico.
10. `resume_errors_on_collision` — mock retorna que un handle ya existe y el nodo emite el error de colisión.
11. `resume_does_not_log_real_values` — captura `tracing` durante la ejecución y verifica que los valores reales no aparecen.

### Integration test (en `tests/`)

Un grafo mínimo con dos secretos en un solo `secure_suspend`:

1. Ejecuta el nodo. La sesión queda SUSPENDED.
2. Reanuda con `--answer "<q1>\n<val1>\n<q2>\n<val2>"`. Verifica que el output downstream tiene los dos handles esperados.
3. Encadena un `http_request` apuntando a un mock server local cuyo body usa los dos handles. Verifica que el body que llegó al mock server contiene `<val1>` y `<val2>` reales (la inyección funcionó).

Marcado `#[ignore = "requires DATABASE_URL — run with \`cargo test -- --ignored\`"]`.

### Test de tool-call → suspend (verificación, no implementación)

Antes de cerrar el spec necesito verificar que **una llamada a un tool desde `llm_call` que retorna SUSPENDED propaga el suspend correctamente al nivel del DAG**. Si esto no funciona, el patrón completo se cae y el spec es inviable. Plan:

- Buscar tests existentes en `tests/` o `src/.../llm.rs` que cubran el caso "tool retorna SUSPENDED dentro de llm_call".
- Si no existen, agregar uno como parte de este spec antes de implementar `secure_suspend` — sería un test de regresión sobre el motor, pero usando `SuspendNode` actual como tool. Si el test pasa con `SuspendNode`, también funcionará con `secure_suspend`.

Esto se confirma en el plan de implementación, no aquí.

## Pre-requisitos / fuera de alcance

**Pre-requisitos confirmados:**

- Colmena y ADP comparten `DATABASE_URL` y la misma tabla de secure values.
- El cliente que invoca el DAG (CLI o `/chat/run` de ADP) propaga `--answer` / `prompt` como `__colmena_resume_answer` al reanudar.

**Fuera de alcance de este spec:**

- Promoción de secretos a un scope mayor que `session_id` (workspace, environment). Si las pruebas confirman que se necesita, se aborda en un spec separado.
- Cambios al UX de ADP para detectar `questions[].type == "secret"` y renderizar input enmascarado. Esto es una mejora pero no bloquea el funcionamiento (el valor sigue siendo seguro porque jamás vuelve al LLM).
- Limpieza/expiración explícita de handles. Ya existe `SecureValueRepository::cleanup_expired`; este spec asume que la política actual es suficiente.
- **Inyección por substring dentro de strings más largas.** El `inject_secrets` actual solo reemplaza valores que son exactamente un placeholder (`<sv_…>`), no busca dentro de cadenas. **No es una limitación real en este flujo**: el meta-agente usa `api_explorer` para leer el OpenAPI/Swagger del endpoint antes de construir el `apiCall`, así que conoce la forma exacta del request (qué campos espera el body, qué headers, qué query params) y coloca los handles directamente en los campos correspondientes como valores completos. La descripción del tool LLM refuerza esta convención. Si más adelante aparece un caso donde el campo legítimo es un string concatenado (raro), se aborda en un spec aparte sobre `SecureValueService`.

## Cambios concretos al repo

| Archivo | Acción |
|---|---|
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/secure_suspend.rs` | **Nuevo**. ~150 LoC: struct, impl `ExecutableNode`, módulo de tests. |
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/mod.rs` | `pub mod secure_suspend;` |
| `src/libs/colmena/src/dag_engine/infrastructure/registry.rs` | Registrar `"secure_suspend"` con su factory. La factory recibe el `Arc<dyn SecureValueRepository>` del service container. |
| `src/libs/colmena/src/shared/service_container.rs` | Verificar que el repo de secure values ya está disponible para los node factories; si no, exponerlo. |
| `tests/graphs/basic/secure_suspend_smoke.json` | Grafo mínimo para el integration test. |
| `tests/secure_suspend_integration.rs` | Integration test descrito arriba (`#[ignore]`). |
| `docs/node_configurations.json` | Agregar la entrada del nuevo node_type con sus campos. |
| `docs/agent_context/node_ports_reference.md` | Listar puertos/outputs del nuevo nodo. |

No se modifica `SuspendNode`, `SecureValueService`, ni el motor del DAG.

## Próximos pasos tras aprobación

1. Pasar al spec writing-plans para producir un plan de implementación detallado de este spec.
2. Implementar siguiendo el plan, con TDD por la naturaleza de seguridad del cambio.
3. Validar el integration test contra una DB Postgres real (`source .env && cargo test -- --ignored`).
4. Solo entonces avanzar a Spec 3 (catálogo) y Spec 2 (par de grafos canvas-builder), que ya pueden depender de este nodo registrado.
