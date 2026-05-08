# Diseño: `inject_secrets` aplica también sobre `config` del nodo

**Estado:** propuesta aprobada
**Fecha:** 2026-05-07
**Autor:** Daniel García (Startti)

## Contexto

Hoy el motor del DAG llama `SecureValueService::inject_secrets` justo antes de ejecutar cada nodo no-LLM, pero **solo recorre los `inputs`** del nodo:

```rust
// src/libs/colmena/src/dag_engine/application/run_use_case.rs:~381
if let Err(e) = svc.inject_secrets(&mut inputs_value, &session_id).await {
    ...
}
```

Esto deja al `config` del nodo intacto. Para flujos donde los handles llegan via tool calls (caso LLM tool / `dag_tool_executor.rs`) eso es suficiente — los handles aterrizan en inputs y se reemplazan correctamente. Pero para el patrón canónico que va a producir el meta-agente del canvas-builder — un nodo top-level del grafo cuyo `config` contiene handles, p. ej:

```jsonc
{
  "type": "http_request",
  "config": {
    "body": { "user": "<sv_demo_user>", "password": "<sv_demo_password>" },
    "secure": true
  }
}
```

…los handles llegan literalmente al servidor remoto porque nunca se inyectan. Validado empíricamente en la prueba real `tests/graphs/advanced/secure_suspend_login_direct.json` (httpbin echó el body literal con `<sv_demo_user>`/`<sv_demo_password>`).

## Objetivo

Extender el run_use_case para que invoque `inject_secrets` sobre **la `config` del nodo además de los `inputs`**, antes de cada ejecución, en sesiones donde hay `SecureValueService` configurado.

## Diseño

### Cambio único

En `src/libs/colmena/src/dag_engine/application/run_use_case.rs`, en el bloque que hoy hace inject de inputs (~línea 381), agregar inmediatamente después un bloque idéntico para `node_config.config`. Pseudocódigo:

```rust
if let Err(e) = svc.inject_secrets(&mut inputs_value, &session_id).await { ... }
// NUEVO:
if let Err(e) = svc.inject_secrets(&mut node_config.config, &session_id).await { ... }
```

`SecureValueService::inject_secrets` ya acepta cualquier `Value` y solo reemplaza strings cuya forma exacta sea `<...>` y que existan en la tabla. Así que aplicarla sobre config:

- No interfiere con `${ENV_VAR}` (sintaxis distinta — env vars usan `${...}`, los handles `<...>`).
- No reemplaza strings literales que casualmente lookean tipo `<...>` pero NO existen en la DB (la función hace lookup; sin match → no reemplaza).
- Es idempotente.

### Lo que NO cambia

- El dispatcher de tools (`dag_tool_executor.rs`) **no se toca**. Ya funciona porque los args del LLM aterrizan en inputs, no en config.
- `SecureValueService` no cambia.
- El nodo HTTP / SQL / etc no cambian.

### Orden de inyección

`inputs` primero, luego `config`. No hay dependencia entre ellos. El orden es estable y predecible.

## Casos de uso cubiertos después del fix

1. **Canvas-builder genera un canvas node con handles en `config.body`** → al ejecutarse el agente nuevo, los handles se reemplazan antes del HTTP. **Caso primario.**
2. **Grafos directos JSON con handles en config** (como `secure_suspend_login_direct.json`) → funcionan sin necesidad de cablear edges con dotted paths.
3. **Tools LLM con handles en `fixed_config` o en partes fijas de `node_schema`** → ya funcionan hoy (van a inputs via merge); este cambio no los afecta negativamente.

## Plan de testing

### Unit test (en `secure_value_service.rs` — opcional, ya cubierto)

`SecureValueService::inject_secrets` ya tiene cobertura unitaria — no necesita más.

### Test de integración (caso canónico)

Un grafo nuevo `tests/graphs/basic/secure_value_in_config_smoke.json` (mucho más simple que el e2e existente):

- Persistir un valor manualmente (vía `repo.persist`) bajo `<sv_smoke>` con valor `"smoke-value-xyz"`.
- Ejecutar un grafo con un solo `log` node cuya config tenga `{ "anything": "<sv_smoke>" }`.
- Verificar que el log node ve el valor real, no el handle.

Marcado `#[ignore = "requires DATABASE_URL"]`.

### Re-validación end-to-end

Re-correr `tests/graphs/advanced/secure_suspend_login_direct.json` v2 (con `body: {user: "<sv_demo_user>", ...}` en config) y verificar que:

- El `node-end` del HTTP node muestra el response de httpbin con `json: {"user":"juan@example.com", "password":"my-Real-PWD-987"}` (valores reales) — no `<sv_...>`.
- Los `args` (query params) ya no contienen los valores duplicados (porque ahora el body sí se construye correctamente).

## Pre-requisitos / fuera de alcance

**Pre-requisito:** ninguno. Cambio aislado.

**Fuera de alcance:**

- No tocar el orden global de pre-procesamiento del nodo (env var resolution, etc).
- No agregar un mecanismo opt-out por nodo. La inyección es siempre segura porque solo reemplaza strings que están en la DB.
- LLM nodes: la `config` de un `llm_call` también pasaría por inject. En la práctica el `system_message`, `api_key`, etc. no contienen handles, pero si por alguna razón los contuvieran, se reemplazarían — comportamiento esperado y deseable.

## Cambios concretos al repo

| Archivo | Acción |
|---|---|
| `src/libs/colmena/src/dag_engine/application/run_use_case.rs` | Agregar 4-6 líneas: segundo bloque `inject_secrets` sobre la config del nodo, junto al existente sobre inputs. |
| `tests/graphs/basic/secure_value_in_config_smoke.json` | Grafo mínimo nuevo. |
| `tests/secure_value_in_config_integration.rs` | Test de integración nuevo, `#[ignore]`. |
