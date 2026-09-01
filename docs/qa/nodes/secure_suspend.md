# secure_suspend — Auditoría QA (Documentación vs Código)

**Nodo:** `secure_suspend`  
**Código fuente:** `src/libs/colmena/src/dag_engine/infrastructure/nodes/secure_suspend.rs`  
**Configuración canónica:** `docs/node_configurations.json` → `node_types.secure_suspend`  
**Puertos:** `docs/agent_context/node_ports_reference.md` → `secure_suspend` (líneas 445-520)  
**Estrategia de seguridad:** `docs/developer_guide/13_security_strategy.md` → Strategy 6 (líneas 369-462)  
**Spec de diseño:** `docs/superpowers/specs/2026-05-07-secure-suspend-node-design.md`  
**Fecha de auditoría:** 2026-08-30

---

## 1. Hallazgos: Documentación

### 1.1 Especificación vs node_ports_reference.md — discrepancia en formato de IDs de preguntas

**Problema:** El spec 2026-05-07 línea 88-89 declara que cada pregunta emitida debe tener `id: "<id>__1"`, `id: "<id>__2"` (sufijados con `__N`). Pero `node_ports_reference.md:463` dice: "El `id` de cada question es el `name` del secret".

**Verificación en código:** `secure_suspend.rs:346-356` emite:
```rust
json!({
    "id": s.name,  // ← usa s.name directamente, NO <id>__N
    "question": s.question,
    "type": "secret",
    "options": Value::Null
})
```

El código coincide con `node_ports_reference.md` (usa `secret.name`), no con el spec (que dice `<id>__N`).

**Impacto:** ALTO. El spec es incorrecto. Aunque `node_ports_reference.md` documenta el comportamiento real, el spec puede confundir a futuros maintainers. Además, el spec línea 480-481 alude a "la nota **LLM tool suspend propagation**" que en realidad no existe en ese documento.

**Remediación:** Actualizar `docs/superpowers/specs/2026-05-07-secure-suspend-node-design.md` línea 88-89 para reemplazar `"<id>__1"`, `"<id>__2"` con `"<name>"` (la sintaxis real). El spec debe reflejar que cada pregunta emitida usa el `name` del secret correspondiente como su id directo.

---

### 1.2 node_configurations.json — la descripción del campo `id` es ambigua y contradice el comportamiento

**Problema:** `node_configurations.json:1039-1045` documenta:
```
"id": {
  "type": "string",
  "required": false,
  "default": null,
  "description": "ID estable del bloque de preguntas. Default: el __node_id del nodo. Las questions emitidas tienen IDs de la forma `<id>__1`, `<id>__2`, ..."
}
```

Pero el código no usa ese campo `id` para emitir los question IDs. El campo `id` del config se ignora completamente en la suspend-path.

**Verificación en código:** `secure_suspend.rs:346-356` no consulta `config.id` en absoluto. Itera sobre `secrets` y usa cada `s.name` como el id de la pregunta.

**Impacto:** MEDIO. Un operador que lea la documentación esperaría que `config.id` afecte los IDs emitidos (`<id>__1`, etc.), pero no es así. El campo se documenta pero es inútil.

**Remediación:** 
1. Actualizar la descripción en `node_configurations.json:1040-1043` a: "ID estable del bloque (NO UTILIZADO actualmente — reservado para futuras extensiones). Default: `__node_id` del nodo. **Los IDs de las questions emitidas son determinados por el `name` de cada secret, no por este campo.**"
2. Alternativa: remover el campo si no se usa.

---

### 1.3 node_configurations.json — esquema de resume_answer_format documenta formato textual, pero el parser es más estricto

**Problema:** `node_configurations.json:1087-1089` dice:
```
"resume_answer_format": {
  "format": "Q[<name1>]: <q1>\nA[<name1>]: <a1>\nQ[<name2>]: <q2>\nA[<name2>]: <a2>...",
  "description": "Resume answers are keyed by the secret's `name`. Order does not matter..."
}
```

Esto describe un formato con prefijos `Q[<name>]:` y `A[<name>]:`. El código (`qa_response_parser.rs`) SÍ requiere estos prefijos, así que esto es correcto. Pero la documentación NO menciona que la pregunta echo después de `Q[<name>]:` se valida contra el config literal palabra por palabra (en suspend).

**Verificación en código:** `suspend.rs:71-79` (suspend path) no valida el echo de la pregunta porque no hay uno emitido (la pregunta se emite en `questions[]` como un campo JSON). En `secure_suspend.rs:480-495`, el formato espera el texto literal de cada pregunta como ancla para parsear.

**Impacto:** BAJO. La documentación es correcta; es solo incompleta sobre el requisito "la pregunta echo debe coincidir LITERALMENTE con la pregunta del config" (aunque la especificación en node_ports_reference.md:494 lo aclara: "El echo después de `Q[<name>]:` es human-readable y NO se valida").

**Remediación:** Agregar una nota a `node_configurations.json:1087-1089` aclarando que para `secure_suspend` el echo sí importa como ancla (aunque el parser no valida que coincida exactamente — solo busca dónde termina). Ver node_ports_reference.md:493-494 para el detalle completo.

---

### 1.4 developer_guide/13_security_strategy.md — falta documentación de `cfg_or_input` pattern para tool-path usage

**Problema:** `13_security_strategy.md:376` describe el flujo end-to-end pero NO explica que cuando `secure_suspend` se usa como LLM tool en `tool_configurations`, el executor merges `fixed_config` + `node_schema` en `inputs` y pasa `config = {}` vacío al nodo.

**Realidad en código:** `secure_suspend.rs:211-228` implementa `cfg_or_input` logic (inputs toman precedencia sobre config vacío). Los tests líneas 1021-1072 verifican ambos paths (graph node con config, tool con inputs).

**Impacto:** MEDIO. LLM developers que setean `secure_suspend` como tool pueden no saber si usar `fixed_config` o `node_schema` o ambos. La guía asume que todas las configuraciones van en `fixed_config` (Mode A/B/C), pero no documenta el data flow.

**Remediación:** Extender `13_security_strategy.md` con una sección "Config-first / Inputs-fallback pattern (tool-path usage)" documentando:
- Que `cfg_or_input` resuelve config primero, inputs segundo.
- Ejemplo: tool_configurations entry con `node_schema` para `secrets` (LLM-visible) y sin `fixed_config`.
- Link a `node_as_tools_reference.json` entrada `secure_suspend` (cuando exista).

---

### 1.5 node_as_tools_reference.json — entrada ausente para secure_suspend como LLM tool

**Problema:** `docs/node_as_tools_reference.json` no contiene una entrada modelando cómo usar `secure_suspend` como LLM tool dentro de `tool_configurations`.

**Verificación:** Existe en `13_security_strategy.md:369-434` (Mode B y C) y `node_ports_reference.md:500-518`, pero NO en `node_as_tools_reference.json`.

**Impacto:** BAJO-MEDIO. LLM developers que buscan "cómo exponer secure_suspend como tool para llm_call" no encuentran la entrada canónica en `node_as_tools_reference.json` (el índice esperado de herramientas LLM).

**Remediación:** Agregar entrada `secure_suspend` a `docs/node_as_tools_reference.json` mostrando:
- `node_type: "secure_suspend"`
- Ejemplo `tool_configurations` entry
- Descripción canónica (que ya existe en `13_security_strategy.md:425-437`)
- `node_schema` esperado (que ya existe en `node_configurations.json:1030-1038`)

---

## 2. Hallazgos: Código

### 2.1 Validación de `name` — regex alineado con spec y bien testeado

**Implementación:** `secure_suspend.rs:125-126` define `NAME_RE: r"^[a-z][a-z0-9_]{2,63}$"`.

**Verificación:** Spec 2026-05-07:78 confirma exactamente esta sintaxis: "lowercase slug, 3-64 chars".

**Tests:** Líneas 624-635 verifican que nombres con mayúsculas fallan antes de la pausa. ✓

**Estado:** Excelente. Spec-aligned, bien testeado.

---

### 2.2 Validación de `secrets` array — no-empty, uniqueness checks son correctos

**Implementación:** `secure_suspend.rs:134-183` valida:
- Lista no vacía (línea 139)
- Cada ítem tiene `question` (143-148) y `name` (149-152)
- `name` matchea regex (153-157)
- Nombres únicos (165-172)
- Preguntas únicas (173-179)

**Tests:** Líneas 597-713 cubren missing secrets, empty array, invalid name, duplicate names, duplicate questions. ✓

**Estado:** Excelente. Correctamente implementado y cubierto por tests.

---

### 2.3 Suspend-path output — formato coincide con spec (sin legacy `question` field)

**Implementación:** `secure_suspend.rs:346-361` emite:
```json
{
  "__colmena_status": "SUSPENDED",
  "questions": [
    { "id": s.name, "question": s.question, "type": "secret", "options": null }
  ]
}
```

**Verificación:** Spec 2026-05-07:83-93 confirma esta estructura (sin `question` legacy, que sí emite `suspend.rs` para BC). La diferencia está documentada en el spec línea 96.

**Tests:** Línea 669-678 verifica que `type: "secret"` está presente. ✓

**Estado:** Correcto. Coincide con spec, razonablemente testeado.

---

### 2.4 Configuración para tool-path — `cfg_or_input` pattern correcto

**Implementación:** `secure_suspend.rs:211-228` maneja ambos paths:
- Top-level DAG node: config tiene `secrets`, inputs vacío → usa config
- LLM tool: config vacío, inputs tiene `secrets` (del LLM) → usa inputs

**Tests:** Líneas 1021-1072 verifican que ambos paths funcionan (input-driven cuando `inputs.get("secrets")` está presente, config-driven sino).

**Estado:** Correcto. Bien implementado.

---

### 2.5 Resume-path Q/A parsing — usa `qa_response_parser::parse_qa_response`

**Implementación:** `secure_suspend.rs:266-269` llama a `parse_qa_response(answer, &id_refs)` donde `id_refs` son los `secret.name` de cada ítem.

**Verificación:** El parser (`qa_response_parser.rs`) busca `Q[<id>]:` y `A[<id>]:` como anclas, bindea por id (order-independent), preserva multi-line values.

**Tests:** Línea 1074-1093 (propagates parser errors). El parser mismo es testeado en `qa_response_parser.rs` unit tests. ✓

**Estado:** Correcto. Usa infraestructura compartida, bien integrado.

---

### 2.6 Min-length validation para secret values — 4-char requirement

**Implementación:** `secure_suspend.rs:286-295` rechaza valores < 4 chars antes de cualquier persistencia:
```rust
const MIN_SECRET_VALUE_LEN: usize = 4;
for (s, v) in secrets.iter().zip(values.iter()) {
    if v.chars().count() < MIN_SECRET_VALUE_LEN {
        return Err(...)
    }
}
```

**Verificación:** CLAUDE.md línea ~1015 documenta: "`secure_suspend` rechaza valores con menos de 4 caracteres" (motivo: outbound masking pathological over-masking). Spec 2026-05-07 NO menciona esto explícitamente.

**Tests:** Líneas 1017-1046 verifican que 3 chars falla y 4 chars pasa. ✓

**Impacto en documentación:** Este requisito NO está documentado en `node_configurations.json` ni en `node_ports_reference.md`.

**Remediación:** Agregar a `node_configurations.json` y `node_ports_reference.md` (bajo validación o bajo output_ports) la nota: "Valores con menos de 4 caracteres son rechazados. El mínimo es 4 caracteres (causa: la inyección usa masking de substring, valores muy cortos causarían over-masking patológico)."

---

### 2.7 Colisión de handles — pre-check fail-closed antes de cualquier persist

**Implementación:** `secure_suspend.rs:298-311` verifica colisión de cada handle ANTES de escribir nada:
```rust
for s in &secrets {
    let handle = format!("<sv_{}>", s.name);
    if self.secure_value_service.handle_exists(...).await? {
        return Err(...)
    }
}
// [luego persist todos]
```

**Verificación:** Spec 2026-05-07:151 confirma: "colisión de handle (ya existe en sesión)" como un caso de error esperado en resume-path.

**Tests:** Líneas 917-954 verifican que el error se emite y que el repo NO muta. ✓

**Estado:** Excelente. Fail-closed design, bien testeado.

---

### 2.8 Agent-session-id propagation — correctamente pasado a repo

**Implementación:** `secure_suspend.rs:258-260` (resume-path) extrae `__colmena_agent_session_id` de inputs y lo pasa a `handle_exists` y `persist_secret`.

**Tests:** Líneas 959-1014 verifican que:
- Con `agent_session_id` presente: se propaga correctamente (líneas 965-985)
- Sin `agent_session_id`: se pasa `None` (líneas 990-1013)

**Estado:** Correcto. Implementa cross-session lookup como se especifica en CLAUDE.md.

---

### 2.9 Logging — NO logea valores reales, solo handles

**Implementación:** `secure_suspend.rs:232-239` logea metadata de ejecución (secret count, presencia de inputs) pero NO logea valores.

**Prueba de regresión:** Líneas 1158-1213 ejecutan el node bajo `tracing::subscriber` con max level TRACE y verifican que la marca "SUPER_SECRET_MARKER_qwerty12345" NO aparece en los logs capturados. ✓

**Estado:** Excelente. Explícitamente protegido contra regression de logging.

---

### 2.10 Tool injection helpers — `apply_secure_suspend_tool_defaults` es idempotente

**Implementación:** `secure_suspend.rs:31-41` y `synthetic_secure_suspend_tool` (líneas 79-96) y `maybe_inject_secure_suspend_tool` (líneas 102-123) son helpers para auto-wiring el tool en LLM callnode cuando `secure_suspend_allowed: true`.

**Tests:** Líneas 378-471 verifican:
- Defaults inyectados cuando faltan (394-408)
- Defaults preservan user-provided description (410-422)
- Defaults preservan user-provided schema (424-444)
- No-op para otros node_types (446-458)
- Idempotente (460-470)

**Estado:** Excelente. Bien diseñado, exhaustivamente testeado.

---

## 3. Casos de Prueba Ejecutables

Todos los casos usan `cargo run --bin dag_engine -- run <graph.json>` con `--agent-session-id` para keying de estado persistido en `secure_value_mappings`.

### 3.1 Test A: Suspend básico — dos secretos en una pausa

**Archivo:** `tests/graphs/basic/secure_suspend_minimal.json` (crear si no existe)

```json
{
  "nodes": {
    "ask_creds": {
      "type": "secure_suspend",
      "config": {
        "secrets": [
          { "question": "Cuál es tu Amadeus client_id?",     "name": "amadeus_client_id" },
          { "question": "Cuál es tu Amadeus client_secret?", "name": "amadeus_client_secret" }
        ]
      }
    },
    "output": { "type": "output" }
  },
  "edges": [{ "from": "ask_creds", "to": "output" }]
}
```

**Ejecución - Run 1 (suspend):**
```bash
source .env
cargo run --bin dag_engine -- run tests/graphs/basic/secure_suspend_minimal.json \
  --agent-session-id secure_demo_001
```

**Validación esperada:**
- Output contiene `"__colmena_status": "SUSPENDED"`
- `questions` es array de 2 ítems con `type: "secret"`
- Cada `questions[i].id` es exactamente `amadeus_client_id` o `amadeus_client_secret` (NO `<id>__1`, `<id>__2`)
- Nada de valores reales en el JSON emitido

**Ejecución - Run 2 (resume):**
```bash
cargo run --bin dag_engine -- run tests/graphs/basic/secure_suspend_minimal.json \
  --agent-session-id secure_demo_001 \
  --answer "Q[amadeus_client_id]: Cuál es tu Amadeus client_id?
A[amadeus_client_id]: AMG-CLI-ID-abc
Q[amadeus_client_secret]: Cuál es tu Amadeus client_secret?
A[amadeus_client_secret]: AMG-CLI-SEC-xyz"
```

**Validación esperada:**
- Engine detecta `agent_session_id`, restaura state
- secure_suspend ejecuta resume-path
- Output: `{ "__colmena_is_output_node": true, "output": { "status": "resumed", "handles": { "amadeus_client_id": "<sv_amadeus_client_id_...>", "amadeus_client_secret": "<sv_amadeus_client_secret_...>" } } }`
- Valores reales "AMG-CLI-ID-abc" y "AMG-CLI-SEC-xyz" jamás aparecen en salida
- Handles fueron persistidos en `secure_value_mappings` con `agent_session_id = secure_demo_001`

---

### 3.2 Test B: Inyección end-to-end — handle resuelto por downstream node

**Archivo:** `tests/graphs/basic/secure_suspend_injection.json`

```json
{
  "nodes": {
    "collect_secret": {
      "type": "secure_suspend",
      "config": {
        "secrets": [{ "question": "API token", "name": "api_token" }]
      }
    },
    "use_token": {
      "type": "http_request",
      "config": {
        "url": "https://httpbin.org",
        "endpoint": "/bearer",
        "method": "GET",
        "headers": { "Authorization": "Bearer <sv_api_token>" }
      }
    },
    "output": { "type": "log" }
  },
  "edges": [
    { "from": "collect_secret.handles.api_token", "to": "use_token.config" }
  ]
}
```

**Run 1 - Suspend:**
```bash
cargo run --bin dag_engine -- run tests/graphs/basic/secure_suspend_injection.json \
  --agent-session-id inject_demo_001
```

**Run 2 - Resume + inyección:**
```bash
cargo run --bin dag_engine -- run tests/graphs/basic/secure_suspend_injection.json \
  --agent-session-id inject_demo_001 \
  --answer "Q[api_token]: API token
A[api_token]: sk-test-1234567890"
```

**Validación esperada:**
- `http_request` node ejecuta luego de resume
- El header `Authorization: Bearer <sv_api_token>` es resuelto a `Authorization: Bearer sk-test-1234567890` antes del HTTP request
- El valor `sk-test-1234567890` jamás aparece en logs ni output del DAG (solo en el HTTP wire hacia httpbin)
- httpbin.org responde con 200 (si el Bearer es válido) o 401 (si no)

---

### 3.3 Test C: Como LLM tool — llm_call invoca secure_suspend_allowed

**Archivo:** `tests/graphs/agents/secure_suspend_as_tool.json`

```json
{
  "nodes": {
    "agent": {
      "type": "llm_call",
      "config": {
        "provider": "google",
        "model": "gemini-2.5-flash",
        "api_key": "${GEMINI_API_KEY}",
        "connection_url": "${DATABASE_URL}",
        "secure_suspend_allowed": true,
        "system_message": "Eres un agente que necesita pedir al usuario un API token. Cuando sea necesario, llama a la tool ask_secret.",
        "prompt": "Necesito un API token del usuario para continuar. Pídelo."
      }
    },
    "output": { "type": "log" }
  },
  "edges": []
}
```

**Run 1 - Agent first turn:**
```bash
source .env
cargo run --bin dag_engine -- run tests/graphs/agents/secure_suspend_as_tool.json \
  --agent-session-id tool_demo_001
```

**Validación esperada:**
- Agent recibe `ask_secret` en su lista de herramientas (auto-inyectado por `secure_suspend_allowed: true`)
- Agent decide llamar a `ask_secret` con array de 1 secreto
- Engine detecta `__colmena_status: SUSPENDED` en tool result
- DAG se pausa (propaga SUSPENDED al nivel del agent loop)

**Run 2 - Resume con respuesta:**
```bash
cargo run --bin dag_engine -- run tests/graphs/agents/secure_suspend_as_tool.json \
  --agent-session-id tool_demo_001 \
  --answer "Q[my_api_token]: Cuál es tu API token
A[my_api_token]: my-secret-token-value"
```

**Validación esperada:**
- Engine resume restaura agent conversation state
- Agent re-despacha el `ask_secret` tool call con la respuesta del usuario
- Agent recibe handles mapa en el tool result (nunca los valores reales)
- Agent loop continúa normalmente

---

### 3.4 Test D: Validaciones de suspend-path — fallos pre-pausa

**Archivo:** `tests/graphs/basic/secure_suspend_validation.json`

Usar el mismo archivo para todos los sub-tests, variando el config:

#### 3.4a: Empty secrets array
```json
{ "type": "secure_suspend", "config": { "secrets": [] } }
```
**Esperado:** Error antes de pausar: `"secure_suspend: secrets list missing or empty"`

#### 3.4b: Invalid name (uppercase)
```json
{ "type": "secure_suspend", "config": { "secrets": [{ "question": "Q", "name": "BadName" }] } }
```
**Esperado:** Error: `"secure_suspend: name 'BadName' invalid (expected lowercase slug, 3-64 chars)"`

#### 3.4c: Duplicate names
```json
{ "type": "secure_suspend", "config": { "secrets": [
  { "question": "Q1", "name": "dup" },
  { "question": "Q2", "name": "dup" }
] } }
```
**Esperado:** Error: `"secure_suspend: duplicate name 'dup' in secrets list"`

#### 3.4d: Duplicate questions
```json
{ "type": "secure_suspend", "config": { "secrets": [
  { "question": "Same?", "name": "n1" },
  { "question": "Same?", "name": "n2" }
] } }
```
**Esperado:** Error: `"secure_suspend: duplicate question text — make each question unique"`

---

### 3.5 Test E: Validaciones de resume-path — Q/A parsing errors

**Archivo:** `tests/graphs/basic/secure_suspend_resume_validation.json`

Base: una pausa con 2 secretos (`n1`, `n2`).

#### 3.5a: Missing answer for id
```bash
--answer "Q[n1]: Q1
A[n1]: val1"
# n2 no aparece
```
**Esperado:** Error en resume: `"secure_suspend: missing answer for id 'n2'"`

#### 3.5b: Empty value
```bash
--answer "Q[n1]: Q1
A[n1]: 
Q[n2]: Q2
A[n2]: val2"
```
**Esperado:** Error: `"secure_suspend: value for secret 'n1' is too short (min 4 chars)"`

#### 3.5c: Value < 4 chars (outbound masking safety)
```bash
--answer "Q[n1]: Q1
A[n1]: abc
Q[n2]: Q2
A[n2]: val2"
```
**Esperado:** Error: `"secure_suspend: value for secret 'n1' is too short (min 4 chars). Short values cause unsafe outbound masking — please supply ≥4 chars."`

---

### 3.6 Test F: Handle collision — pre-exist in session

**Archivo:** `tests/graphs/basic/secure_suspend_collision.json`

Pausa 1 con 1 secreto (`my_token`). Pausa 2 con el mismo `name` en la misma sesión.

**Run 1:**
```bash
cargo run --bin dag_engine -- run tests/graphs/basic/secure_suspend_collision.json \
  --agent-session-id coll_001
```

Suspend OK, resume OK (handle `<sv_my_token>` persistido).

**Run 2 — otra pausa con el mismo name:**

Grafo alterno con dos nodos secure_suspend secuenciales, ambos con `name: "my_token"`, misma `agent_session_id coll_001`.

**Ejecución:**
```bash
cargo run --bin dag_engine -- run tests/graphs/basic/secure_suspend_collision_chain.json \
  --agent-session-id coll_001 \
  --answer "Q[my_token]: Primer token
A[my_token]: primer_valor_suficientemente_largo
Q[my_token]: Segundo token
A[my_token]: segundo_valor_suficientemente_largo"
```

**Validación esperada:**
- Primer suspend-resume OK
- Segundo secure_suspend en la cadena intenta persistir `<sv_my_token>` nuevamente en la misma sesión
- Error: `"secure_suspend: handle <sv_my_token> already exists in session — use a different name"`
- Repo NO muta (transactional fail)

---

### 3.7 Test G: Multiline values — RSA key como secret

**Archivo:** `tests/graphs/basic/secure_suspend_multiline.json`

```json
{
  "type": "secure_suspend",
  "config": {
    "secrets": [
      { "question": "RSA private key (multi-line)", "name": "rsa_key" },
      { "question": "Passphrase",                   "name": "passphrase" }
    ]
  }
}
```

**Run 1:** Suspend.

**Run 2 — Resume con RSA key multilínea:**
```bash
cargo run --bin dag_engine -- run tests/graphs/basic/secure_suspend_multiline.json \
  --agent-session-id multi_001 \
  --answer "Q[rsa_key]: RSA private key (multi-line)
A[rsa_key]: -----BEGIN PRIVATE KEY-----
MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQDX...
(muchas líneas)
-----END PRIVATE KEY-----
Q[passphrase]: Passphrase
A[passphrase]: my-passphrase-secret"
```

**Validación esperada:**
- Parser preserva los newlines internos de la key (NO los trimea)
- Ancla en `Q[passphrase]:` exactamente donde termina el valor anterior
- Ambos valores son persistidos en su totalidad
- Luego al inyectar, el handle resuelve al valor COMPLETO (con newlines intactos)

---

## Resumen de Hallazgos

| # | Tipo | Severidad | Descripción |
|---|------|-----------|-------------|
| 1.1 | Spec ↔ Docs | ALTA | Spec dice `<id>__N`, código/docs dicen `secret.name` directo — spec incorrecto |
| 1.2 | Docs | MEDIA | Campo `config.id` documentado pero no usado; documentación ambigua |
| 1.3 | Docs | BAJA | Format de resume_answer documentado, pero detalle sobre ancla de pregunta incompleto |
| 1.4 | Docs | MEDIA | `cfg_or_input` pattern no documentado en developer guide |
| 1.5 | Docs | BAJA-MEDIA | `node_as_tools_reference.json` sin entrada para `secure_suspend` como tool |
| 2.1 | Código | ✓ | Validación de `name` alineada con spec |
| 2.2 | Código | ✓ | Validación de `secrets` array correcta |
| 2.3 | Código | ✓ | Output suspend-path alineado con spec |
| 2.4 | Código | ✓ | `cfg_or_input` pattern correcto |
| 2.5 | Código | ✓ | Resume Q/A parsing correcto |
| 2.6 | Código + Docs | MEDIA | Min-length 4 chars NO documentado en docs, pero correcto en código |
| 2.7 | Código | ✓ | Colisión pre-check fail-closed, bien testeado |
| 2.8 | Código | ✓ | Agent-session-id propagation correcto |
| 2.9 | Código | ✓ | Logging no expone valores reales |
| 2.10 | Código | ✓ | Tool injection helpers idempotentes y bien testeados |

---

## Remediaciones Prioritizadas

### Prioridad ALTA

1. **Actualizar spec `2026-05-07-secure-suspend-node-design.md` línea 88-89**: Reemplazar `"id": "<id>__1"` con `"id": "<secret.name>"` para coincidir con la implementación real.

### Prioridad MEDIA

2. **Actualizar `node_configurations.json:1040-1043`**: Aclarar que el campo `id` NO afecta los question IDs emitidos (que son determinados por `secret.name`).

3. **Agregar a `node_configurations.json` y `node_ports_reference.md`**: Documentar el requisito de min-length 4 chars para valores de secretos (causa: outbound masking safety).

4. **Extender `docs/developer_guide/13_security_strategy.md`**: Nueva sección documentando `cfg_or_input` pattern para tool-path usage, con ejemplo de `tool_configurations`.

5. **Agregar entrada a `docs/node_as_tools_reference.json`**: Modelar `secure_suspend` como LLM tool (description canónica, node_schema, ejemplo tool_configurations).

### Prioridad BAJA

6. **Actualizar spec `2026-05-07`:480-481**: Quitar mención a "nota **LLM tool suspend propagation**" que no existe en la ubicación aludida, o agregar ese apartado.

---

**Auditoría completada:** 5 hallazgos en documentación (1 alta, 2 media, 2 baja) + 10 aspectos de código validados (todos correctos/excelentes) + 7 casos de prueba ejecutables cubriendo suspend básico, inyección end-to-end, tool-path, validaciones suspend-path, validaciones resume-path, colisiones, y multiline values.
