# 🚀 Guía de Desarrollo: `dag_engine`

Este documento describe la arquitectura y el proceso de desarrollo para el `dag_engine`, un motor de ejecución de Grafos Acíclicos Dirigidos (DAG) extensible, implementado en Rust y basado en una arquitectura hexagonal limpia.

## 🚀 Conceptos Clave

El motor está diseñado para ejecutar un DAG definido en un fichero JSON.

### El Fichero `graph.json`

Este fichero JSON es el "código fuente" para el motor. Define tres elementos clave:

1.  **`nodes`**: Un mapa de todas las operaciones en el grafo. Cada nodo tiene un ID único (ej. `"start_data"`, `"add_step"`) y define:
    *   **`type`**: Un string (ej. `"add"`, `"log"`) que se mapea a una implementación específica en Rust.
    *   **`config`**: Un objeto JSON para valores estáticos que el nodo necesita (ej. un exponente, un prompt, una URL).

2.  **`edges`**: Una lista de conexiones que definen el flujo de datos.
    *   **`from`**: El origen de los datos, usando una sintaxis similar a JSON-pointer (ej. `"node_id.field_a"` o `"node_id.output"`).
    *   **`to`**: El destino de los datos (ej. `"other_node.input_b"`).

### Flujo de Datos

- El motor ejecuta los nodos en un orden determinado por un **ordenamiento topológico**.
- La salida de un nodo se pasa a la entrada del siguiente, según lo definido en los `edges`.
- Todos los nodos estándar (matemáticos, de log, etc.) deben devolver su resultado envuelto en una clave `output`, por ejemplo: `{ "output": 75.0 }`.
- Los nodos raíz (como `mock_input`) son especiales y emiten su objeto `config` directamente como salida.

## 🏛️ Arquitectura: Hexagonal (Puertos y Adaptadores)

El motor sigue una estricta arquitectura hexagonal, separando la lógica en tres capas distintas. Esto hace que el sistema sea altamente modular y fácil de testear y extender.

### 1. `domain` (El Núcleo)

Es el corazón de la aplicación. Es Rust puro y no tiene dependencias del "mundo exterior" (como bases de datos, APIs o nuestro `main.rs`).

-   **`domain/graph.rs`**: Define las estructuras de datos puras (`Graph`, `NodeConfig`, `Edge`).
-   **`domain/node.rs`**: Define el "Puerto" principal (el trait `ExecutableNode`). Este es el contrato central que todos los nodos deben firmar. Básicamente dice: "Debes ser capaz de ejecutar".
-   **`domain/error.rs`**: Define los errores puros del dominio (`DagError`, ej. `CycleDetected`).

### 2. `application` (El Orquestador)

Esta capa contiene la "lógica de negocio" de cómo ejecutar un grafo. Depende del `domain` pero no sabe nada sobre cómo se implementan los nodos.

-   **`application/ports.rs`**: Define los "Puertos" que la aplicación necesita del mundo exterior (ej. el trait `NodeRegistryPort`, que dice "Necesito una forma de encontrar un nodo a partir de su `type` string").
-   **`application/run_use_case.rs`**: Es el cerebro del motor.
    -   Recibe el `NodeRegistryPort` mediante inyección de dependencias.
    -   Realiza el ordenamiento topológico para obtener el orden de ejecución.
    -   Itera a través de los nodos.
    -   Construye los `NodeInputs` para cada nodo parseando los `edges`.
    -   Usa el `NodeRegistryPort` para obtener la implementación correcta del nodo.
    -   Llama a `node.execute()`.

### 3. `infrastructure` (El Mundo "Real")

Esta capa implementa todos los "Puertos" definidos en las capas `domain` y `application`. Aquí es donde ocurre todo el trabajo "sucio".

-   **`infrastructure/nodes/`**: Contiene todas nuestras implementaciones de nodos (ej. `AddNode`, `LogNode`, `ExponentialNode`). Cada uno de estos es un "Adaptador" que implementa el trait `ExecutableNode`.
-   **`infrastructure/registry.rs`**: Es el "Adaptador" que implementa el `NodeRegistryPort`. `HashMapNodeRegistry` usa un simple `HashMap` para conectar strings (ej. `"add"`) con la estructura concreta `AddNode`.
-   **`main.rs`**: Es el "Adaptador Primario" o "Ensamblador". Inicializa el `HashMapNodeRegistry`, lo inyecta en el `DagRunUseCase`, y luego le indica al caso de uso que se ejecute.

## 🔧 Cómo Crear un Nuevo Nodo (Ejemplo: `ExponentialNode`)

Crear un nuevo nodo es la forma principal de extender el motor. Es un proceso simple de dos pasos.

### Paso 1: Implementar el Trait `ExecutableNode`

Primero, crea la estructura de tu nodo e implementa el trait `ExecutableNode`. Añadiremos esto a `infrastructure/nodes/math.rs`.

-   **Leer de `inputs`**: Usa `inputs.get("input_name")` para obtener datos de los `edges` entrantes.
-   **Leer de `config`**: Usa `config.get("config_key")` para obtener configuración estática.
-   **Devolver Salida**: Devuelve tu resultado envuelto en `json!({ "output": ... })`.

```rust
// en: src/dag_engine/infrastructure/nodes/math.rs

// ... (otros imports) ...

// --- ExponentialNode ---
pub struct ExponentialNode;
#[async_trait::async_trait]
impl ExecutableNode for ExponentialNode {
    async fn execute(&self, inputs: &NodeInputs, config: &Value, _state: &mut Value) -> Result<Value, Box<dyn StdError>> {
        // 1. Obtener la base del edge entrante "input"
        let base = get_f64(inputs.get("input"), "input")?;
        
        // 2. Obtener el exponente del "config" del nodo
        let exponent = get_f64(config.get("exponent"), "config.exponent")?;

        // 3. Calcular y devolver el resultado envuelto
        let result = base.powf(exponent);
        Ok(json!({ "output": result }))
    }

    fn schema(&self) -> Value { 
        json!({
            "type": "exponential", 
            "inputs": {"input": "number"}, 
            "config": {"exponent": "number"},
            "outputs": {"output": "number"}
        }) 
    }
}
```

### Paso 2: Registrar el Nodo

Segundo, "inyecta" tu nuevo nodo en la aplicación añadiéndolo al registro.

Abre `src/dag_engine/infrastructure/registry.rs` y añade tu nodo en la función `HashMapNodeRegistry::new()`.

```rust
// en: src/dag_engine/infrastructure/registry.rs

// ... (otros registros de nodos) ...
nodes.insert("divide".to_string(), Arc::new(DivideNode));

// --- Añadir el nuevo nodo ---
nodes.insert("exponential".to_string(), Arc::new(ExponentialNode));
        
Self { nodes }
```

¡Eso es todo! El motor ahora es consciente de tu tipo de nodo `exponential`.

## 📊 Ejemplo `graph.json` (para `test_pow.json`)

Este grafo demuestra cómo usar el nodo `mock_input` para proporcionar datos iniciales y el nodo `exponential` para leer tanto de un `edge` (`input`) como de su propia `config`.

```json
{
  "nodes": {
    "start": {
      "type": "mock_input",
      "config": {
        "base_num": 5
      }
    },
    "pow_step": {
      "type": "exponential",
      "config": {
        "exponent": 3
      }
    },
    "log_result": {
      "type": "log"
    }
  },
  "edges": [
    {
      "from": "start.base_num",
      "to": "pow_step.input"
    },
    {
      "from": "pow_step.output",
      "to": "log_result.input"
    }
  ]
}
```

## Cómo Ejecutar

Recuerda especificar tu binario (`dag_engine`) al ejecutar:

```bash
cargo run --bin dag_engine ./test_pow.json
```
