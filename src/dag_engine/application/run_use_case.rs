use crate::dag_engine::application::ports::NodeRegistryPort;
use crate::dag_engine::domain::error::DagError;
use crate::dag_engine::domain::graph::{Edge, Graph};
use crate::dag_engine::domain::node::NodeInputs;

use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

/// El "Caso de Uso" que orquesta la ejecución de un grafo.
/// Es agnóstico a la infraestructura (no sabe de dónde vienen los nodos).
#[derive(Clone)]
pub struct DagRunUseCase {
    /// El "Puerto" inyectado que nos da acceso a las implementaciones
    /// concretas de los nodos.
    registry: Arc<dyn NodeRegistryPort>,
}

impl DagRunUseCase {
    /// Constructor para inyectar las dependencias (Puertos).
    pub fn new(registry: Arc<dyn NodeRegistryPort>) -> Self {
        Self { registry }
    }

    /// Método principal que ejecuta el grafo.
    pub async fn execute(&self, graph: Graph) -> Result<Value, DagError> {
        // 1. Obtener el orden de ejecución y detectar ciclos.
        let execution_order = self.topological_sort(&graph)?;

        let mut global_state = Value::Null; // Estado global (usado en M2)

        // Almacén para todas las salidas de los nodos.
        // La clave es el "node_id" (ej. "node_a"),
        // el valor es el `Value` que ese nodo produjo.
        let mut all_outputs: HashMap<String, Value> = HashMap::new();

        // 2. Iterar y ejecutar cada nodo en orden.
        for node_id in &execution_order {
            let node_config = graph
                .nodes
                .get(node_id)
                // Esto no debería fallar si topo_sort es correcto, pero es buena práctica.
                .ok_or_else(|| DagError::NodeIdNotFound(node_id.clone()))?;

            // 3. Obtener la implementación concreta del nodo desde el registro.
            let node_impl = self
                .registry
                .get_node(&node_config.node_type)
                .ok_or_else(|| DagError::NodeTypeNotFound(node_config.node_type.clone()))?;

            // 4. Construir el `NodeInputs` para este nodo.
            let inputs = self.build_inputs_for(node_id, &graph.edges, &all_outputs)?;

            // 5. ¡Ejecutar la lógica del nodo!
            let output = node_impl
                .execute(&inputs, &node_config.config, &mut global_state, None)
                .await
                .map_err(|e| DagError::NodeExecution(e.to_string()))?;

            // 6. Almacenar la salida del nodo para que los nodos futuros la usen.
            all_outputs.insert(node_id.to_string(), output);
        }

        // Retornar la salida del último nodo *en el orden de ejecución*.
        if let Some(last_node_id) = execution_order.last() {
            // Obtiene la salida del último nodo (ej. "log_step") del mapa
            Ok(all_outputs
                .get(last_node_id)
                .cloned()
                .unwrap_or(Value::Null))
        } else {
            // El grafo estaba vacío
            Ok(Value::Null)
        }
    }

    /// Executes the graph and streams events for each step.
    pub fn execute_stream(
        self,
        graph: Graph,
    ) -> impl futures::Stream<
        Item = Result<crate::dag_engine::domain::events::DagExecutionEvent, DagError>,
    > {
        async_stream::try_stream! {
            use crate::dag_engine::domain::events::DagExecutionEvent;
            use crate::dag_engine::domain::observer::NodeEvent;

            // 1. Obtener el orden de ejecución y detectar ciclos.
            let execution_order = self.topological_sort(&graph)?;

            let mut global_state = Value::Null;
            let mut all_outputs: HashMap<String, Value> = HashMap::new();

            // 2. Iterar y ejecutar cada nodo en orden.
            for node_id in &execution_order {
                let node_config = graph
                    .nodes
                    .get(node_id)
                    .ok_or_else(|| DagError::NodeIdNotFound(node_id.clone()))?;

                // 3. Obtener implementación
                let node_impl = self
                    .registry
                    .get_node(&node_config.node_type)
                    .ok_or_else(|| DagError::NodeTypeNotFound(node_config.node_type.clone()))?;

                // 4. Construir inputs
                let inputs = self.build_inputs_for(node_id, &graph.edges, &all_outputs)?;

                // Yield Start Event
                yield DagExecutionEvent::NodeStart {
                    node_id: node_id.clone(),
                    node_type: node_config.node_type.clone(),
                    inputs: serde_json::to_value(&inputs).unwrap_or(Value::Null),
                    config: node_config.config.clone(),
                };

                // Create channel for observer
                let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
                let observer = Arc::new(ChannelObserver { tx });

                // 5. Ejecutar (CONCURRENTLY with event draining)
                // We wrap execution in a future
                let execution_future = node_impl.execute(&inputs, &node_config.config, &mut global_state, Some(observer));
                tokio::pin!(execution_future);

                let output_result = loop {
                    tokio::select! {
                        res = &mut execution_future => {
                            break res;
                        }
                        Some(event) = rx.recv() => {
                            match event {
                                NodeEvent::LlmToken { token } => {
                                    yield DagExecutionEvent::LlmToken {
                                        node_id: node_id.clone(),
                                        token
                                    };
                                }
                                NodeEvent::LlmToolCall { tool_id, tool_name, args_chunk } => {
                                    yield DagExecutionEvent::LlmToolCall {
                                        node_id: node_id.clone(),
                                        tool_id,
                                        tool_name,
                                        args_chunk,
                                    };
                                }
                                NodeEvent::LlmUsage { prompt_tokens, completion_tokens } => {
                                    yield DagExecutionEvent::LlmUsage {
                                        node_id: node_id.clone(),
                                        prompt_tokens,
                                        completion_tokens,
                                    };
                                }
                                NodeEvent::LlmToolCallStart { tool_id, tool_name, tool_args } => {
                                    yield DagExecutionEvent::LlmToolCallStart {
                                        node_id: node_id.clone(),
                                        tool_id,
                                        tool_name,
                                        tool_args,
                                    };
                                }
                                NodeEvent::LlmToolCallFinish { tool_id, success, output } => {
                                    yield DagExecutionEvent::LlmToolCallFinish {
                                        node_id: node_id.clone(),
                                        tool_id,
                                        success,
                                        output,
                                    };
                                }
                            }
                        }
                    }
                };

                let output = output_result.map_err(|e| DagError::NodeExecution(e.to_string()))?;

                // Yield Finish Event
                yield DagExecutionEvent::NodeFinish {
                    node_id: node_id.clone(),
                    output: output.clone(),
                };

                // 6. Almacenar
                all_outputs.insert(node_id.to_string(), output);
            }

            // Yield Graph Finish Event
            let final_output = if let Some(last_node_id) = execution_order.last() {
                all_outputs
                    .get(last_node_id)
                    .cloned()
                    .unwrap_or(Value::Null)
            } else {
                Value::Null
            };

            yield DagExecutionEvent::GraphFinish { output: final_output };
        }
    }

    /// Implementa el algoritmo de Kahn para ordenamiento topológico.
    /// También detecta ciclos.
    fn topological_sort(&self, graph: &Graph) -> Result<Vec<String>, DagError> {
        let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();
        let mut in_degree: HashMap<&str, i32> = HashMap::new();

        // Inicializar `in_degree` para todos los nodos
        for node_id in graph.nodes.keys() {
            in_degree.entry(node_id).or_insert(0);
            adj.entry(node_id).or_default();
        }

        // Construir la lista de adyacencia y los grados de entrada
        for edge in &graph.edges {
            // Parseamos "from" y "to" para obtener solo los IDs de nodo
            let from_node = edge.from.split('.').next().unwrap_or("");
            let to_node = edge.to.split('.').next().unwrap_or("");

            if !graph.nodes.contains_key(from_node) {
                return Err(DagError::NodeIdNotFound(from_node.to_string()));
            }
            if !graph.nodes.contains_key(to_node) {
                return Err(DagError::NodeIdNotFound(to_node.to_string()));
            }

            adj.entry(from_node).or_default().push(to_node);
            *in_degree.entry(to_node).or_default() += 1;
        }

        // Cola para el algoritmo de Kahn
        let mut queue: VecDeque<&str> = VecDeque::new();
        for (node_id, &degree) in &in_degree {
            if degree == 0 {
                queue.push_back(node_id);
            }
        }

        let mut order = Vec::new();
        while let Some(u) = queue.pop_front() {
            order.push(u.to_string());

            if let Some(neighbors) = adj.get(u) {
                for &v in neighbors {
                    if let Some(degree) = in_degree.get_mut(v) {
                        *degree -= 1;
                        if *degree == 0 {
                            queue.push_back(v);
                        }
                    }
                }
            }
        }

        // Si el orden no incluye a todos los nodos, hay un ciclo.
        if order.len() != graph.nodes.len() {
            Err(DagError::CycleDetected)
        } else {
            Ok(order)
        }
    }

    /// Construye el `NodeInputs` (HashMap) para un nodo específico,
    /// resolviendo todos sus bordes (`edges`) de entrada.
    fn build_inputs_for(
        &self,
        current_node_id: &str,
        all_edges: &[Edge],
        all_outputs: &HashMap<String, Value>,
    ) -> Result<NodeInputs, DagError> {
        let mut inputs: NodeInputs = HashMap::new();

        // Encontrar todos los bordes que apuntan A este nodo
        let incoming_edges = all_edges
            .iter()
            .filter(|edge| edge.to.starts_with(current_node_id));

        for edge in incoming_edges {
            // `edge.to`   -> "current_node_id.input_name"
            // `edge.from` -> "source_node_id.output_name.field"

            let parts_to: Vec<&str> = edge.to.splitn(2, '.').collect();
            if parts_to.len() != 2 {
                continue;
            } // Borde mal formado
            let input_name = parts_to[1]; // ej. "a", "b", "prompt"

            let parts_from: Vec<&str> = edge.from.splitn(2, '.').collect();
            if parts_from.is_empty() {
                continue;
            } // Borde mal formado

            let source_node_id = parts_from[0];

            // Obtener el `Value` de salida completo del nodo fuente
            let source_output_value = all_outputs
                .get(source_node_id)
                // Si el output no está listo, es un error de grafo (debería estarlo por el topo-sort)
                .ok_or_else(|| DagError::NodeIdNotFound(source_node_id.to_string()))?;

            // Ahora, resolvemos el valor específico
            let value_to_pass = if parts_from.len() == 1 {
                // El `from` era solo "source_node_id", pasamos el output completo
                source_output_value.clone()
            } else {
                // El `from` era "source_node_id.output_name" o "source_node_id.field_a.field_b"
                // Usamos un puntero JSON para seleccionar el sub-campo
                let json_pointer = parts_from[1].replace('.', "/");
                source_output_value
                    .pointer(&format!("/{}", json_pointer))
                    .cloned()
                    .unwrap_or(Value::Null) // Si el campo no existe, pasa Null
            };

            inputs.insert(input_name.to_string(), value_to_pass);
        }

        Ok(inputs)
    }
}

/// Observer that sends events to an mpsc channel
struct ChannelObserver {
    tx: tokio::sync::mpsc::UnboundedSender<crate::dag_engine::domain::observer::NodeEvent>,
}

impl crate::dag_engine::domain::observer::ExecutionObserver for ChannelObserver {
    fn on_event(&self, event: crate::dag_engine::domain::observer::NodeEvent) {
        let _ = self.tx.send(event);
    }
}
