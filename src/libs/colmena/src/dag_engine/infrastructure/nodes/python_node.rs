use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use async_trait::async_trait;
use pyo3::prelude::*;
use pyo3::types::PyDict;
use pythonize::{depythonize_bound, pythonize};
use serde_json::{json, Value};
use std::error::Error as StdError;
use std::sync::Arc;

pub struct PythonNode;

#[async_trait]
impl ExecutableNode for PythonNode {
    async fn execute(
        &self,
        inputs: &NodeInputs,
        config: &Value,
        _state: &mut Value,
        _observer: Option<Arc<dyn crate::dag_engine::domain::observer::ExecutionObserver>>,
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        // 1. Validar Configuración: Extraer el código
        // Prioridad: 1. Inputs ("code"), 2. Config ("code")
        let code = if let Some(input_code) = inputs.get("code").and_then(|v| v.as_str()) {
            input_code.to_string()
        } else {
            config
                .get("code")
                .and_then(|v| v.as_str())
                .ok_or("PythonNode error: 'code' field is missing in inputs or config")?
                .to_string()
        };

        // Clean up markdown code blocks if present (LLMs often wrap code in ```python ... ```)
        let code = code.trim();
        let code = if code.starts_with("```") {
            // Find the first newline (after ```python or ```)
            let start = code.find('\n').map(|i| i + 1).unwrap_or(0);
            // Find the last ``` delimiter
            let end = code.rfind("```").unwrap_or(code.len());
            code[start..end].trim()
        } else {
            code
        };

        // Debug logging to help diagnose issues
        println!("[PythonNode] Executing code:\n{}", code);

        // 2. Preparar Inputs: Clonar para mover al closure
        let inputs_clone = inputs.clone();
        let code = code.to_string(); // Convert &str to String for the closure

        // 3. Ejecución Bloqueante:
        // CPython no es thread-safe con el runtime async de Tokio.
        // Usamos spawn_blocking para mover la carga pesada y el GIL a un thread dedicado.
        let output_json = tokio::task::spawn_blocking(move || -> Result<Value, String> {
            Python::with_gil(|py| {
                // A. Crear entorno local (diccionario de variables)
                let locals = PyDict::new_bound(py);

                // B. Inyectar Inputs como variables globales en el script
                // Ejemplo: si inputs tiene {"x": 10}, en python existirá la variable `x`
                for (key, value) in inputs_clone {
                    let py_val = pythonize(py, &value).map_err(|e| {
                        format!("Failed to convert input '{}' to Python: {}", key, e)
                    })?;
                    locals
                        .set_item(&key, py_val)
                        .map_err(|e| format!("Failed to set input '{}': {}", key, e))?;
                }

                // C. Ejecutar el código arbitrario
                // Se ejecuta como un módulo '__main__' virtual
                // IMPORTANT: We pass locals as both globals AND locals so that:
                // 1. Function definitions are added to the namespace
                // 2. Those functions can be called within the same script
                py.run_bound(&code, Some(&locals), Some(&locals))
                    .map_err(|e| format!("Python execution error: {}", e))?;

                // D. Extraer Resultados
                // Convención: El script debe guardar el resultado en una variable llamada 'output'
                // Si 'output' no existe, devolvemos null.
                match locals.get_item("output") {
                    Ok(Some(output_obj)) => {
                        let json_output: Value = depythonize_bound(output_obj).map_err(|e| {
                            format!("Failed to convert Python 'output' to JSON: {}", e)
                        })?;
                        Ok(json_output)
                    }
                    _ => Ok(Value::Null),
                }
            })
        })
        .await??; // Primer ? es para JoinError (Tokio), segundo ? es para el Result interno

        // 4. Retornar estructura estándar
        Ok(json!({
            "output": output_json
        }))
    }

    fn schema(&self) -> Value {
        json!({
            "name": "python_script",
            "description": "Executes Python code. Code can be provided via input 'code' or config 'code'. Inputs are injected as variables. Assign result to variable 'output'.",
            "config": {
                "code": {
                    "type": "string",
                    "description": "Python script to execute (fallback if not in inputs)"
                }
            },
            "inputs": {
                "code": {
                    "type": "string",
                    "description": "Python script to execute (optional, overrides config)"
                },
                "description": "Key-value pairs injected as global variables"
            },
            "outputs": {
                "output": "The value of the 'output' variable from the script"
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[tokio::test]
    async fn test_python_math_logic() {
        let node = PythonNode;

        // 1. Configurar Inputs: x = 10, y = 5
        let mut inputs = HashMap::new();
        inputs.insert("x".to_string(), json!(10));
        inputs.insert("y".to_string(), json!(5));

        // 2. Configurar Script: Multiplicar y guardar en 'output'
        let config = json!({
            "code": "output = x * y + 2"
        });

        let mut state = json!({});

        // 3. Ejecutar
        let result = node
            .execute(&inputs, &config, &mut state, None)
            .await
            .unwrap();

        // 4. Aserción: (10 * 5) + 2 = 52
        assert_eq!(result["output"], 52);
    }

    #[tokio::test]
    async fn test_python_imports() {
        // Verificar que podemos importar librerías estándar (json, math)
        let node = PythonNode;
        let inputs = HashMap::new();

        let config = json!({
            "code": "import math\noutput = math.sqrt(16)"
        });

        let mut state = json!({});
        let result = node
            .execute(&inputs, &config, &mut state, None)
            .await
            .unwrap();

        assert_eq!(result["output"], 4.0);
    }
}
