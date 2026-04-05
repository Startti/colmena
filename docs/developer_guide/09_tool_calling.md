# 🛠️ Uso de Herramientas (Tool Calling)

Colmena permite a los agentes LLM utilizar "herramientas" para interactuar con el mundo exterior. Estas herramientas son nodos del DAG pre-configurados que el LLM puede invocar dinámicamente.

## Configuración de Herramientas en el DAG

Expones nodos del DAG (como `http_request`) como herramientas para el LLM mediante `tool_configurations` en el nodo `llm_call`.

### Ejemplo básico: Nodo HTTP como Herramienta

```json
{
  "nodes": {
    "agent": {
      "type": "llm_call",
      "config": {
        "provider": "openai",
        "api_key": "${OPENAI_API_KEY}",
        "model": "gpt-4o-mini",
        "system_message": "Eres un asistente útil. Usa las herramientas disponibles.",
        "enabled_tools": ["fetch_users", "create_user"],
        "tool_configurations": {
          "fetch_users": {
            "node_type": "http_request",
            "description": "Obtener datos de usuarios de la API.",
            "fixed_config": {
              "base_url": "https://jsonplaceholder.typicode.com",
              "endpoint": "/users",
              "method": "GET"
            }
          },
          "create_user": {
            "node_type": "http_request",
            "description": "Crear un nuevo usuario. Proporciona nombre, email y teléfono.",
            "fixed_config": {
              "base_url": "https://jsonplaceholder.typicode.com",
              "endpoint": "/users",
              "method": "POST",
              "headers": { "Content-Type": "application/json" }
            }
          }
        }
      }
    }
  }
}
```

### Cómo Funciona

1. **`fixed_config`** — parámetros que el LLM no ve ni puede modificar (URL, método, credenciales).
2. **`node_schema`** — forma moderna de definir qué parámetros expone el LLM y cuáles son fijos. Reemplaza `fixed_config` + `exposed_inputs`.
3. **`DagToolExecutor`** combina los argumentos del LLM con los valores fijos y ejecuta el nodo.
4. El resultado se devuelve al LLM para que continúe la conversación.

---

## 🔒 Herramientas HTTP con Secure Values

Cuando una herramienta HTTP tiene `"secure": true` en su `fixed_config`, **`DagToolExecutor` aplica `hash_output()` ANTES de devolver el resultado al LLM**. Esto garantiza que el LLM nunca vea tokens reales.

### Flujo de seguridad en tools:

```
LLM llama get_amadeus_token
  ↓
HttpNode ejecuta POST → Amadeus responde {access_token: "real_xyz"}
  ↓
DagToolExecutor detecta secure: true
  ↓
hash_output() → {access_token: "<value_1>"}, encripta real en DB
  ↓
LLM recibe {access_token: "<value_1>"} ← nunca ve el token real ✅
  ↓
LLM llama search_flights con bearer_token: "<value_1>"
  ↓
DagToolExecutor llama inject_secrets() → reemplaza "<value_1>" con token real
  ↓
HttpNode ejecuta GET con Authorization: Bearer real_xyz ✅
```

### Ejemplo completo: LLM controla autenticación + búsqueda (patrón Amadeus)

Este patrón fue validado en `tests/graphs/agents/amadeus_llm_http_auth_experiment.json`:

```json
{
  "nodes": {
    "travel_agent": {
      "type": "llm_call",
      "config": {
        "provider": "openai",
        "model": "gpt-4o-mini",
        "api_key": "${OPENAI_API_KEY}",
        "system_message": "Para buscar vuelos: (1) llama get_amadeus_token, (2) usa el placeholder retornado en search_flights.",
        "enabled_tools": ["get_amadeus_token", "search_flights"],
        "tool_configurations": {
          "get_amadeus_token": {
            "name": "get_amadeus_token",
            "node_type": "http_request",
            "description": "Autenticar con Amadeus. Retorna un token como <value_1>.",
            "fixed_config": {
              "base_url": "https://api.amadeus.com/v1/security/oauth2",
              "endpoint": "/token",
              "method": "POST",
              "headers": { "Content-Type": "application/x-www-form-urlencoded" },
              "body": "grant_type=client_credentials&client_id=${AMADEUS_CLIENT_ID}&client_secret=${AMADEUS_CLIENT_SECRET}",
              "secure": true
            }
          },
          "search_flights": {
            "name": "search_flights",
            "node_type": "http_request",
            "description": "Buscar vuelos. Usar el placeholder del token en bearer_token.",
            "node_schema": {
              "base_url": { "type": "string", "fixed": "https://api.amadeus.com" },
              "endpoint": { "type": "string", "fixed": "/v2/shopping/flight-offers" },
              "method": { "type": "string", "fixed": "GET" },
              "bearer_token": {
                "type": "string",
                "required": true,
                "description": "Token obtenido de get_amadeus_token. Pasar el placeholder exacto."
              },
              "query_params": {
                "type": "object",
                "properties": {
                  "originLocationCode": { "type": "string", "required": true },
                  "destinationLocationCode": { "type": "string", "required": true },
                  "departureDate": { "type": "string", "required": true },
                  "adults": { "type": "string", "required": true }
                }
              }
            }
          }
        }
      }
    }
  }
}
```

**Requisitos:** `DATABASE_URL`, `SECURE_VALUES_KEY`, `AMADEUS_CLIENT_ID`, `AMADEUS_CLIENT_SECRET`, `OPENAI_API_KEY`.

```bash
# Ejecutar el experimento
set -a && source .env && set +a
cargo run --bin dag_engine -- run tests/graphs/agents/amadeus_llm_http_auth_experiment.json
```

---

## Tipos de Body en Herramientas HTTP

El `HttpNode` distingue body de query params **por la clave del campo**, no por el método HTTP:

| Clave | Comportamiento | Equivalente curl |
|---|---|---|
| `"body": "string"` | Raw text body (URL-encoded, GraphQL, etc.) | `--data-raw "..."` |
| `"body": { }` | JSON body automático | `-d '{...}'` con `Content-Type: application/json` |
| `"query_params": { }` | Query parameters en la URL | `?key=val&...` |
| `"bearer_token": "..."` | Header `Authorization: Bearer ...` | `-H "Authorization: Bearer ..."` |
| `"headers": { }` | HTTP headers arbitrarios | `-H "Key: Val"` |

> **Importante:** El `Content-Type: application/x-www-form-urlencoded` para OAuth2 debes setearlo explícitamente en `headers`. El nodo no lo infiere del formato del body string.

### Campos internos que NUNCA se envían como query params

La lista `reserved_keys` en `HttpNode` filtra estos campos del mecanismo de `extra_params`:

```
body, query_params, query_parameters, headers, base_url, endpoint, method,
bearer_token, authorization, secure, __colmena_session_id, __node_id, __colmena_resume_answer
```

> **Bug corregido (2026-04-05):** `"secure": true` fue añadido a `reserved_keys`. Antes se filtraba como `?secure=true` a APIs externas causando errores 400.

---

## Ejecución

Cuando el LLM decide usar una herramienta, `DagToolExecutor`:
1. Combina los argumentos del LLM con `fixed_config` / `node_schema`.
2. Llama `inject_secrets()` para reemplazar `<value_N>` con valores reales.
3. Ejecuta el nodo.
4. Si `secure: true`, llama `hash_output()` — el LLM recibe placeholders, nunca valores reales.
5. Devuelve el resultado (seguro) al LLM.
