# Diseño: nodo `secure_suspend`

**Estado:** propuesta
**Fecha:** 2026-05-07
**Autor:** Daniel García (Startti)

## Contexto

El meta-agente "canvas builder" (ver `tests/graphs/external/socketio_canvas_builder.json`) construye grafos de ADP para el usuario. Cuando un grafo nuevo necesita autenticarse contra una API externa (HubSpot, Stripe, etc.), el meta-agente debe poner un token en el campo `bearerToken` del nodo `apiCall` que está creando.

Hoy las dos únicas opciones son:

1. Que el meta-agente use `${ENV_VAR}` y delegar al usuario definir esa variable fuera de banda. No es operable: el meta-agente no puede recolectar el secreto en la conversación.
2. Que el meta-agente le pida el token al usuario por chat. El valor pasa por el contexto del LLM y por logs. Inaceptable.

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

### Config schema

```jsonc
{
  "question": "string, required",  // visible al usuario
  "name": "string, required",      // slug del secreto, e.g. \"hubspot_private_token\"
  "id": "string, optional"         // ID estable de la pregunta para mapeo en UIs (default: __node_id)
}
```

Decisiones:

- **`name` es obligatorio** y se usa como sufijo del handle: el output es exactamente `<sv_<name>>`. El meta-agente puede predecirlo y usarlo varias veces si quiere.
- **El `name` debe matchear `^[a-z][a-z0-9_]{2,63}$`**. Si no, el nodo retorna error en suspend-path (antes de pausar). Esto evita handles raros y reduce ambigüedad de inyección.
- **No hay `question_type`** ni `options`. Un secreto siempre es texto libre.
- **No hay `secure: bool`**. Este nodo siempre es seguro; el booleano sería un foot-gun (cf. opción B descartada en brainstorm).

### Salida del nodo

**Suspend-path** (primera ejecución, sin `__colmena_resume_answer`):

```json
{
  "__colmena_status": "SUSPENDED",
  "question": "<la pregunta>",
  "questions": [
    { "id": "<id>", "question": "<la pregunta>", "type": "secret", "options": null }
  ]
}
```

Notar `"type": "secret"`. Las UIs (ADP frontend) pueden detectar este tipo y renderizar un input enmascarado / sin autocompletar. Si la UI no lo distingue, fallback a `text` es seguro porque el valor nunca se logea.

**Resume-path** (con `__colmena_resume_answer`):

```json
{
  "status": "resumed",
  "handle": "<sv_<name>>"
}
```

**Crítico**: el campo `answer_received` de `SuspendNode` NO existe aquí. El valor real jamás aparece en el output. Esto rompe deliberadamente la simetría con `suspend` para forzar que el llamador no pueda accidentalmente leer el secreto.

### Errores

El motor envuelve cualquier error retornado por `execute` en `DagError::NodeExecution(String)`. El nodo retorna mensajes específicos para que sean fácilmente identificables en logs y respuestas:

- `name` ausente o no matchea regex `^[a-z][a-z0-9_]{2,63}$` → `Err("secure_suspend: name missing or invalid format (expected lowercase slug, 3-64 chars)")` en suspend-path, **antes** de emitir SUSPENDED. Esto evita pausar el grafo solo para fallar al reanudarlo.
- Colisión de handle (ya existe `<sv_<name>>` en la misma `session_id`) → `Err("secure_suspend: handle <sv_<name>> already exists in session — use a different name")` en resume-path. Decisión consciente: nunca sobrescribir silenciosamente. Si el meta-agente quiere actualizar, debe usar un `name` distinto o limpiar la sesión.
- Falla al persistir (DB caída) → propaga el error del repo como string. La sesión queda en estado fallido; el motor no resume.

La detección de colisión requiere una operación adicional en `SecureValueRepository`: o bien un `exists(session_id, hash_key) -> bool`, o cambiar `persist` a fallar cuando ya existe (Postgres `INSERT ... ON CONFLICT DO NOTHING` + chequeo de filas afectadas). El plan de implementación elige el camino menos invasivo.

## Uso como tool LLM

El nodo se expone en `tool_configurations` así:

```jsonc
{
  "ask_secret": {
    "name": "ask_secret",
    "node_type": "secure_suspend",
    "description": "Ask the user for a SECRET (API token, password, private key) needed to authenticate against an external service. The user's answer is stored encrypted; you only get back a HANDLE of the form `<sv_<name>>` that you can paste into the bearerToken / Authorization header / any auth field of an apiCall node. NEVER ask the user for secrets via plain text — always use this tool. Compose the question as a short, specific request: name the service and what kind of credential. Examples of good questions: 'Pega el HubSpot private app token (empieza con \"pat-\").', 'Cuál es la API key de Stripe en modo live?'. Bad questions: 'dame tu token', 'qué credencial uso?'.",
    "node_schema": {
      "question": {
        "type": "string",
        "required": true,
        "description": "The exact text shown to the user. Mention the service and the credential type. Do NOT include any example value."
      },
      "name": {
        "type": "string",
        "required": true,
        "description": "Slug identifying this secret in the session. Lowercase letters, digits, underscore. 3–64 chars. Examples: 'hubspot_private_token', 'stripe_live_key'. Use the same slug if you want to reference the same secret again later (collision is an error)."
      }
    }
  }
}
```

Notas importantes:

- La `description` arriba es **prescriptiva**: dice cuándo llamar el tool y cómo redactar la pregunta. Esto es lo que el usuario explícitamente pidió: "cuando se agregue como una tool debe tener una descripción específica de cuándo llamarlo y cómo debe componer las preguntas".
- `node_schema` solo expone `question` y `name`. No hay forma de que el LLM pase un valor de secreto.
- Esta plantilla se incluye en el spec del par de grafos canvas-builder (Spec 2) y en la skill `adp-node-catalog` (Spec 3).

## Flujo end-to-end

```
[meta-agente, llm_call]
   │ piensa: "necesito el HubSpot token para crear el apiCall"
   │ tool_call: ask_secret(question="Pega el HubSpot private app token...", name="hubspot_token")
   ▼
[secure_suspend node, suspend-path]
   │ valida name regex → OK
   │ emite __colmena_status: SUSPENDED + questions:[{type:"secret",...}]
   ▼
[engine] congela DAG, persiste session, retorna a cliente
   ▼
[ADP frontend] renderiza pregunta con input enmascarado
   ▼
[usuario] teclea "pat-na1-abcdef..."
   ▼
[ADP] POST /chat/run con sessionId + prompt = "pat-na1-abcdef..."
   ▼
[engine] reanuda DAG con __colmena_resume_answer = "pat-na1-abcdef..."
   ▼
[secure_suspend node, resume-path]
   │ valida no-colisión de "<sv_hubspot_token>" en session
   │ repo.persist(session_id, node_id, "<sv_hubspot_token>", "pat-na1-abcdef...", "secret")
   │ retorna { status: "resumed", handle: "<sv_hubspot_token>" }
   ▼
[meta-agente, llm_call] continúa
   │ tool_result visible al LLM = { status:"resumed", handle:"<sv_hubspot_token>" }
   │ tool_call: create_canvas_node(node={
   │   type: "apiCall",
   │   data: { config: { bearerToken: "<sv_hubspot_token>", secure: true, ... } }
   │ })
   ▼
[ADP runtime, en una ejecución posterior del agente creado]
   │ engine va a ejecutar el apiCall (un http_request)
   │ inject_secrets recorre inputs, encuentra "<sv_hubspot_token>", llama repo.decrypt
   │ reemplaza el placeholder por "pat-na1-abcdef..." JUSTO antes del request HTTP
   ▼
[http_request] dispara el call con el bearer real, el LLM nunca lo vio
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

1. `suspend_path_emits_secret_question_type` — verifica que el output incluye `questions[0].type == "secret"` y `__colmena_status == "SUSPENDED"`.
2. `suspend_path_validates_name_format` — un `name` con espacios o mayúsculas falla con `InvalidConfig`.
3. `suspend_path_requires_name` — `name` ausente falla.
4. `resume_path_persists_value_and_returns_handle` — con un mock repo, verifica que `persist` se invoca con `(session_id, node_id, "<sv_foo>", "real_value", "secret")` y el output es `{status:"resumed", handle:"<sv_foo>"}`. **Crítico**: assert que el output NO contiene la string del valor real.
5. `resume_path_errors_on_collision` — el mock retorna que el handle ya existe y el nodo emite `Conflict`.
6. `resume_path_does_not_log_real_value` — captura `tracing` durante la ejecución y verifica que el valor real no aparece.

### Integration test (en `tests/`)

Un grafo mínimo que:

1. Ejecuta `secure_suspend` con `--answer "test_secret_xyz"` (la sesión va a estar en estado SUSPENDED, se reanuda con la flag estándar del CLI).
2. Encadena un nodo dummy downstream que recibe el handle y lo devuelve. Verifica que el handle es `<sv_test>`.
3. Ejecuta `inject_secrets` (a través de un `http_request` apuntando a un mock server local) y verifica que el header recibido en el mock server contiene `test_secret_xyz`. Esto valida el ciclo completo.

Marcado `#[ignore = "requires DATABASE_URL — run with \`cargo test -- --ignored\`"]` por la convención del proyecto.

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
