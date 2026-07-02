# 👩‍💻 Guía del Desarrollador — Colmena

Esta guía está dirigida a desarrolladores que quieren contribuir, extender o entender en profundidad el funcionamiento de Colmena.

Las secciones están organizadas por **tema**, no por orden numérico. Los prefijos numéricos en los nombres de archivo se mantienen por compatibilidad con enlaces existentes, pero la navegación recomendada es la que sigue.

---

## 1. Empezar aquí

- [**ONBOARDING**](./ONBOARDING.md) — Camino guiado de 0 → primer DAG corriendo (≈30 min).
- [**Codebase Tour**](./CODEBASE_TOUR.md) — Recorrido módulo a módulo del repo (≈30 min).
- [**Configuración del Entorno**](./developer_guide/02_environment_setup.md) — Toolchain, editor, dependencias.
- [**Convenciones de Código**](./developer_guide/03_coding_conventions.md) — Estilo, nombrado, manejo de errores.

## 2. Arquitectura

- [**Architecture Overview**](./developer_guide/00_architecture_overview.md) — Vista de alto nivel: módulos, capas, dependencias.
- [**Arquitectura Hexagonal**](./developer_guide/01_architecture.md) — Principios Ports & Adapters, estructura de capas, flujo de datos.
- [**Motor DAG**](./developer_guide/12_dag_engine_guide.md) — Cómo el motor parsea, planifica y ejecuta grafos.
- [**Flujo de Datos y Conexiones**](./developer_guide/16_data_flow_guide.md) — Cómo se pasan y transforman los datos entre nodos (ports default, `$ref`, `${ENV}`, edge resolution).
- [**Referencia Técnica**](./developer_guide/17_technical_reference.md) — Esquemas JSON y tipos de datos del sistema.
- [**Flujo de Ejecución de Tools**](./developer_guide/22_tool_execution_flow.md) — Ciclo completo de una tool call LLM (de `node_schema` a la ejecución del nodo backing).

## 3. Calidad: testing, performance, troubleshooting

- [**Testing**](./developer_guide/05_testing.md) — Estrategia, patrones de mocking, comandos (cargo test --verbose vs --lib, `#[ignore]`, deny-warnings).
- [**Testing en Python**](./developer_guide/05a_python_testing.md) — Organización y ejecución de la suite de tests Python.
- [**Motor DAG desde Python**](./developer_guide/48_python_dag.md) — `run_dag`, `validate_graph`, `serve_dag`, `inject_payload`, suspend→resume e introspección del registro vía las bindings PyO3.
- [**Ejemplos de uso en Python**](./examples/python_usage.md) — Guía práctica con fragmentos listos para usar: LLM, streaming, DAG, documents y manejo de errores.
- [**Motor DAG desde Node.js / TypeScript**](./developer_guide/49_typescript_dag.md) — `runDag`, `streamDag`, `validateGraph`, `serveDag`, `DagEvent`, `agentSessionId`/resume e introspección del registro vía las bindings napi-rs.
- [**Ejemplos de uso en TypeScript**](./examples/typescript_usage.md) — Guía práctica con fragmentos listos para usar: LLM, streaming, DAG, documents, errores y diferencias clave respecto a Python.
- [**Performance**](./developer_guide/06a_performance.md) — Cómo medir y optimizar.
- [**Troubleshooting**](./developer_guide/18_troubleshooting.md) — Errores frecuentes del engine y bindings, con causas y fixes.

## 4. Contribuir, CI/CD, deploy

- [**Cómo Contribuir**](./developer_guide/08_contributing.md) — Pull requests, revisiones, branching.
- [**Git Hooks**](./developer_guide/08a_git_hooks.md) — Husky, pre-commit, hooks locales.
- [**CI/CD Guide**](./developer_guide/10_cicd_guide.md) — Pipeline de integración y despliegue continuo.
- [**Branch Protection**](./developer_guide/11_branch_protection_rules.md) — Reglas de protección de ramas y flujo de trabajo.
- [**Deployment**](./developer_guide/07_deployment.md) — Proceso de build y publicación.

## 5. LLM y agentes

- [**Añadir Proveedores LLM**](./developer_guide/04_adding_providers.md) — Tutorial paso a paso para extender el módulo LLM.
- [**Nodo LLM Deep Dive**](./developer_guide/14_llm_deep_dive.md) — Parámetros avanzados, capacidades, configuración exhaustiva.
- [**Tool Calling**](./developer_guide/09_tool_calling.md) — Configuración y uso de tool calling en el DAG.
- [**Lazy Tool Loading**](./developer_guide/29_lazy_tool_loading.md) — Catálogo ligero + `describe_tool` para revelar schemas on-demand.
- [**Load Attachment**](./developer_guide/31_load_attachment.md) — Documentos on-demand dentro del loop LLM (`load_attachment`, `$attachment:<key>`).
- [**Skills**](./developer_guide/24_skills.md) — Paquetes de conocimiento markdown cargados via `load_skill`.
- [**Temporal & Geographic Context**](./developer_guide/35_temporal_geographic_context.md) — Inyección automática de fecha/hora/ubicación/locale al `system_message`.
- [**Subgrafos y agentes anidados**](./developer_guide/19_nested_agents_and_subgraphs.md) — El nodo `subgraph`, aislamiento de sesión, propagación HITL.
- [**Arquitectura del Orchestrator**](./developer_guide/20_orchestrator_architecture.md) — Fases, bridge tasks, HITL suspend/resume, critic feedback, replanning.

## 6. Seguridad

- [**Estrategia de Seguridad / Secure Values**](./developer_guide/13_security_strategy.md) — AES-256-GCM, `secure_suspend`, masking outbound, sliding TTL, `SECURE_VALUES_KEY` (fail-fast).

## 7. Persistencia y datos

- [**Database Schema**](./developer_guide/30_database_schema.md) — Tablas Postgres, migraciones, índices, patrón `agent_session_id`-first.
- [**Memoria y Persistencia**](./developer_guide/15_memory_guide.md) — SQLite y PostgreSQL para agentes con memoria conversacional.
- [**Librería de Documentos**](./developer_guide/27_documents_library.md) — `DocumentRuntime`, `document_*` nodos, IR JSON como source of truth.
- [**Archivos grandes via Files API**](./developer_guide/28_large_files_api.md) — Streaming, cache `provider_file_cache`, estrategias por proveedor.
- [**Attachment GC**](./developer_guide/36_attachment_gc.md) — Binario `attachment_gc` que limpia `conversation_attachments` y blobs TTL'd.

## 8. Nodos

- [**Nodo Socket.IO**](./developer_guide/21_socketio_node.md) — `socketio_request`: ack, wait-event, autenticación.
- [**Nodo SQL Query**](./developer_guide/23_sql_node.md) — Permisos granulares, validación AST + critic opcional, sandbox, RLS, auto-creación de schemas.
- [**Nodos Web**](./developer_guide/25_web_nodes.md) — `http_request` (incluye multipart streaming y OAuth2 nativo), `tavily_client`, `api_explorer`, `browser`.
- [**Nodo Python Script**](./developer_guide/26_python_node.md) — `python_script`: PyO3, sandbox `restricted`, threading.
- [**Multimedia Generation**](./developer_guide/32_multimedia_generation.md) — `image_generation`, `image_edit`, `tts`; storage abstracto con 3 adapters.
- [**Router & Output Parser**](./developer_guide/37_router_and_output_parser.md) — `router` (LLM direct vs extract+rules) y `output_parser` (extracción tipada post-LLM).
- [**CRDT Documents**](./developer_guide/38_crdt_documents.md) — Workbooks colaborativos en tiempo real sobre `yrs::Doc`.
- [**Google Sheets**](./developer_guide/39_gsheets.md) — Integración con la Sheets API; lectura/escritura desde grafos.
- [**Sheets local (CRDT) vs Google Sheets**](./developer_guide/43_sheets_local_vs_gsheets.md) — Comparativa, cuándo elegir cada uno, API write-back unificada.
- [**Google Docs**](./developer_guide/45_gdocs.md) — 35 tools sintéticos para crear, leer, exportar y editar quirúrgicamente Google Docs (content-addressed, multi-tab, co-edit guard, table-cell edits).
- [**Google OAuth (auth para gsheets + gdocs)**](./developer_guide/47_google_oauth.md) — Auth user-scoped vía refresh_token desde `agents@startti.co`. Setup one-time con `colmena_oauth_setup`, env vars, runbook de revocación.
- [**data_run_python**](./developer_guide/48_data_run_python.md) — Tool unificado de movimiento de datos tabulares (CSV/XLSX ↔ Google Sheets ↔ SQL) con sinks output_tables/output_sheets/output_attachments; filas nunca pasan por el LLM.
- [**Nodo Suspend (HITL)**](./developer_guide/44_suspend_node.md) — `suspend` / `secure_suspend`, formato Q/A, resume, patrones canónicos.

## 9. Tools y skills (catálogos y referencias)

- [**Toolkit Packages**](./developer_guide/40_toolkit_packages.md) — Activar muchas tools con un alias; sintaxis de exclusión.
- [**Built-in Tools Index**](./developer_guide/41_builtin_tools_index.md) — Cada tool LLM Rust-native con summary y link a su doc detallada.
- [**Built-in Skills Index**](./developer_guide/42_builtin_skills_index.md) — Cada SKILL.md compilada en el binary, con descripción y link.

## 10. Reference de eventos y configuración

- [**SSE Events Reference**](./sse_events_reference.md) — Todos los eventos que el motor DAG emite sobre el stream SSE.
- [**Node Configurations (JSON canon)**](./node_configurations.json) — Schema canónico de cada nodo (campos, tipos, defaults).
- [**Node as Tools Reference (JSON)**](./node_as_tools_reference.json) — Cómo configurar nodos como tools LLM (`tool_configurations`, `node_schema`, `expose_sub_tools`, ejemplos por tipo).
- [**Node Ports Reference**](./agent_context/node_ports_reference.md) — Puertos default (`default_input`/`default_output`) y outputs por tipo de nodo.

## 11. Diseño (DDS) y backlog

- [**Arquitectura Hexagonal** (DD)](./dds/ARQUITECTURA_HEXAGONAL_GUIA.md) — Diseño Ports & Adapters.
- [**DAG Engine** (DD)](./dds/DAG_ENGINE_DISEÑO.md) — Diseño del motor de grafos.
- [**Secure Values** (DD)](./dds/SECURE_VALUES_DISEÑO.md) — Diseño de la encriptación de secretos.
- [**Variable Resolution** (DD)](./dds/VARIABLE_RESOLUTION_DISEÑO.md) — Diseño de resolución de `$ref`, `${ENV}`, `$DYNAMIC`.
- [**BACKLOG**](./BACKLOG.md) — Items pendientes y descartados.
- [**CHANGELOG 2026-05**](./CHANGELOG_2026-05.md), [**CHANGELOG 2026-06**](./CHANGELOG_2026-06.md) — Historial reciente de cambios.

---

> **Mantenimiento:** Cuando agregues una nueva guía, añadila a la sección temática correspondiente arriba. No es necesario respetar la secuencia numérica de los nombres de archivo — los números son legacy.
