# 👩‍💻 Guía del Desarrollador - Colmena

Esta guía está dirigida a desarrolladores que quieren contribuir, extender o entender en profundidad el funcionamiento de Colmena.

## 📋 Tabla de Contenidos

Esta guía se ha dividido en varias secciones para facilitar su consulta.

1.  [**Arquitectura del Proyecto**](./developer_guide/01_architecture.md): Principios de diseño, estructura de directorios y flujo de datos.
2.  [**Configuración del Entorno**](./developer_guide/02_environment_setup.md): Cómo instalar las herramientas y configurar tu editor.
3.  [**Convenciones de Código**](./developer_guide/03_coding_conventions.md): Estándares de nombrado, documentación y manejo de errores.
4.  [**Añadir Nuevos Proveedores**](./developer_guide/04_adding_providers.md): Tutorial paso a paso para extender la librería.
5.  [**Testing**](./developer_guide/05_testing.md): Estrategia de tests, patrones de mocking y comandos útiles.
6.  [**Estructura de Testing en Python**](./developer_guide/06_estructura_testing_python.md): Organización y ejecución de la suite de tests Python.
7.  [**Performance y Optimización**](./developer_guide/06_performance.md): Consejos para medir y mejorar el rendimiento.
8.  [**Deployment y Distribución**](./developer_guide/07_deployment.md): Proceso de build, CI/CD y publicación.
9.  [**Cómo Contribuir**](./developer_guide/08_contributing.md): Guía para el proceso de Pull Requests y revisiones de código.
10. [**Git Hooks**](./developer_guide/09_git_hooks.md): Configuración de Husky y pre-commit hooks.
11. [**Uso de Herramientas**](./developer_guide/09_tool_calling.md): Configuración y uso de Tool Calling en el DAG.
12. [**CI/CD Guide**](./developer_guide/10_cicd_guide.md): Detalles del pipeline de integración y despliegue continuo.
13. [**Branch Protection Rules**](./developer_guide/11_branch_protection_rules.md): Reglas de protección de ramas y flujo de trabajo de Git.
14. [**Guía del Motor DAG**](./developer_guide/12_dag_engine_guide.md): Detalles técnicos sobre el funcionamiento del motor de grafos.
15. [**Secure Values y Estrategia de Seguridad**](./developer_guide/13_security_strategy.md): Diseño y manejo de secretos aes-256-gcm.
16. [**Deep Dive: Nodo LLM**](./developer_guide/14_llm_deep_dive.md): Parámetros avanzados y capacidades de los modelos de lenguaje.
17. [**Guía de Memoria y Persistencia**](./developer_guide/15_memory_guide.md): Configuración de SQLite y PostgreSQL para agentes con memoria.
18. [**Flujo de Datos y Conexiones**](./developer_guide/16_data_flow_guide.md): Detalles sobre cómo se pasan y transforman los datos entre nodos.
19. [**Referencia Técnica**](./developer_guide/17_technical_reference.md): Esquemas JSON y tipos de datos del sistema.
20. [**Troubleshooting y Errores Comunes**](./developer_guide/18_troubleshooting.md): Guía para resolver fallos típicos en el engine y bindings.
21. [**Agentes Anidados y Sub-Grafos**](./developer_guide/19_nested_agents_and_subgraphs.md): El nodo `subgraph`, aislamiento de sesión, propagación HITL y composición modular de agentes.
22. [**Arquitectura del Orchestrator**](./developer_guide/20_orchestrator_architecture.md): Guía completa del nodo `orchestrator`: fases, bridge tasks, HITL suspend/resume, critic feedback loop y replanning dinámico.
23. [**Nodo Socket.IO**](./developer_guide/21_socketio_node.md): El nodo `socketio_request`: conexión a servidores Socket.IO, emisión de eventos, modos ack y wait-event, autenticación por cookies y ejemplos de uso como herramienta LLM.
24. [**Flujo de Ejecución de Tools**](./developer_guide/22_tool_execution_flow.md): Ciclo completo de una tool call LLM — desde `node_schema` hasta la ejecución final del nodo HTTP o Socket.IO, incluyendo parsing, merge de valores fijos con argumentos del LLM, y resolución de variables.
25. [**Nodo SQL Query**](./developer_guide/23_sql_node.md): El nodo `sql_query`: ejecución de consultas PostgreSQL con control granular de permisos (presets + deny), validación híbrida (reglas estáticas + critic LLM opcional), sandbox schema para funciones, inyección de contexto de BD en tool descriptions, multi-tenant RLS, y reuso de pools vía el `PgPoolRegistry` compartido del `ColmenaEngine`.
26. [**Skills**](./developer_guide/24_skills.md): Paquetes de conocimiento en markdown cargados bajo demanda por el nodo LLM vía el tool sintético `load_skill`. Skills built-in (compiladas con `include_dir!`) y skills del usuario (paths con allowed-dirs whitelist); catálogo inyectado en la descripción del tool; eventos `skill_loaded` en SSE y `skills_used` en el summary final.
27. [**Nodos Web**](./developer_guide/25_web_nodes.md): Introducción al runtime de nodos toolkit (`tavily_client`, `api_explorer`, `browser`): expansión de sub-tools, despacho vía `__sub_tool` e integración con `llm_call`. Esqueleto; los specs A/C/B poblan cada sub-sección.
