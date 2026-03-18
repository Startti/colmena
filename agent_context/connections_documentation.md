# Documentación de Conexiones en el DAG Engine de Colmena

En el archivo de configuración `graph.json`, las conexiones entre los nodos se definen dentro del arreglo `edges` a través de los campos `"from"` (origen) y `"to"` (destino).

El motor de ejecución DAG en Rust resuelve y construye los `inputs` dinámicos de cada nodo usando una notación basada en puntos (`.`) que actúa internamente como un **Puntero JSON** (JSON Pointer).

A continuación, se explican en detalle los mecanismos de conexión disponibles:

## Estructura Básica del DAG
```json
{
  "edges": [
    { "from": "nodo_origen", "to": "nodo_destino" }
  ]
}
```

## 1. Conexión Explícita Puerto a Puerto (El método más común)
Sirve para apuntar el valor exacto de la salida de un nodo directamente a un parámetro específico de la entrada de otro.

**Formato:** `"from": "origen.campo_salida", "to": "destino.campo_entrada"`

**Ejemplo:**
```json
{ "from": "math_node.result", "to": "python_script.x" }
```
- **Qué hace:** El motor revisa todo el objeto JSON que devolvió `math_node`. Luego extrae usando un puntero interno el campo `result` y lo deposita dentro del conjunto de inputs del nodo `python_script` bajo la llave `x`.

## 2. Auto-Flattening (Aplanamiento Automático)
Esta es una característica clave del motor DAG. Si pasas la fuente completa (sin especificar un puerto de salida) hacia un nodo de destino completo (sin especificar un puerto de entrada), el motor **desempaqueta el objeto JSON de origen y lo inyecta como variables directas en el destino**.

**Formato:** `"from": "origen", "to": "destino"`

**Ejemplo:**
```json
{ "from": "extractor", "to": "reactor" }
```
- **Qué hace:** Si el nodo `extractor` como respuesta final devuelve un JSON como `{"temperatura": 24, "clima": "soleado"}`, el nodo `reactor` va a recibir en sus inputs directamente las llaves `temperatura` y `clima`. Esto es ideal cuando el payload que viene de la etapa anterior coincide perfectamente con lo que requiere la etapa actual.

## 3. Asignación de Objeto Completo a un Parámetro
Si el nodo de origen envía todo su JSON completo, pero el nodo destino necesita que todo el objeto se asigne a una sola variable específica, debes usar el punto solo en el destino.

**Formato:** `"from": "origen", "to": "destino.payload"`

**Ejemplo:**
```json
{ "from": "trigger_inicial", "to": "python_node.event_data" }
```
- **Qué hace:** Todo el objeto JSON devuelto por el nodo `trigger_inicial` se pasa de forma intacta, sin desgranar, directamente a la variable `event_data` como entrada en `python_node`. El nodo receptor tendrá un diccionario/objeto con toda la estructura bajo esa única llave.

## 4. Punteros JSON Anidados (Rutas Profundas)
Si la salida de un nodo es un JSON complejo y de varios niveles, puedes usar múltiples puntos del lado de `"from"` para viajar de forma profunda a través del JSON.

**Formato:** `"from": "origen.ruta.hacia.el.valor", "to": "destino.input_requerido"`

**Ejemplo:**
```json
{ "from": "api_request.body.data.token", "to": "llm_node.bearer_token" }
```
- **Qué hace:** Va al nodo `api_request`, extrae la llave `body`, de allí la llave `data`, y por último el campo `token`, extrayendo únicamente ese valor (string/número) para asignarlo a la variable `bearer_token` del nodo LLM receptor.

---

### Resumen de Notación

Es importante interpretar el bloque `"from"` como la indicación de **qué bloque de la salida del nodo anterior se va a enviar**, y el bloque `"to"` como **cómo y en qué espacio de los `inputs` se va a leer dentro de la función `execute`**:

*   **`"origen"`** ➔ Traer todo el Output del nodo.
*   **`"origen.campo"`**  ➔ Traer sólo un campo o ruta JSON del Output.
*   **`"destino"`** ➔ Destruir la jerarquía inicial del JSON recibido y volcar todas sus propiedades para que se inserten como propiedades individuales de lectura directa *(Auto-Flattening)*.
*   **`"destino.campo"`** ➔ Encasillar la estructura recibida, intacta, confinada dentro de la ranura de ese preciso parámetro nombrado.
