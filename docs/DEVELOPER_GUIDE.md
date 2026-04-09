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
