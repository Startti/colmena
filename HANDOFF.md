# Colmena — Session Handoff Document

> **Fecha generada:** 2026-05-12  
> **Autor:** Daniel Garcia  
> **Para:** cualquier agente / cuenta / PC que retome este trabajo  
> **Repositorio:** https://github.com/Startti/colmena  
> **Branch actual:** `develop` (sincronizado con `origin/develop`)

---

## Estado actual del proyecto

Plan C (nodo `api_explorer`) está **100% completo, testeado, mergeado y pusheado** a `origin/develop`.  
El siguiente trabajo pendiente es **Plan B** (`browser` node — chromiumoxide + Browserless).

### Resumen de planes web (en orden de implementación)

| Plan | Nombre | Estado |
|------|--------|--------|
| Unified | Fundación compartida (`web/` module, `SessionRegistry`, `ToolkitNode` trait) | ✅ Completo |
| A | `tavily_client` toolkit node | ✅ Completo |
| C | `api_explorer` toolkit node | ✅ Completo (esta sesión) |
| B | `browser` toolkit node (chromiumoxide + Browserless) | ⏳ Pendiente — **SIGUIENTE** |

---

## Qué se hizo en la última sesión (Plan C)

### Bugs corregidos en `api_explorer`

1. **`fuzzy_match_threshold` demasiado alto** (0.6 → 0.1)  
   - `nucleo-matcher` normaliza scores por longitud de haystack; queries cortas como "add pet" daban ~0.15 contra un umbral de 0.6 → 0 resultados. Fix: default a 0.1.

2. **`params` obligatorio → LLM en loop infinito**  
   - El LLM omitía `params`, recibía "missing params", reintentaba 10 veces. Fix: `params` es ahora opcional (default `{}`); el error muestra los parámetros específicos del endpoint.

3. **Gemini rechazaba `$ref` en tool responses**  
   - Error: `The referenced name #/components/schemas/X does not match to a display_name`  
   - Fix: implementada `resolve_refs()` con detección de ciclos en `web/application/api_spec_use_case.rs`. Los schemas se inlinan antes de enviarse al LLM.

4. **Gemini rechazaba arrays sin `items`**  
   - Fix: añadido `items: Option<Box<ParameterProperty>>` a `ParameterProperty` en `llm/domain/tools.rs`. Inicializado en los ~30 call sites existentes.

5. **Cache con URL dual** (URL-de-entrada vs URL-resuelta)  
   - El mismo spec tenía dos entradas en cache. Fix: cache dual-key indexada por ambas URLs.

6. **`default_base_url_override` no se propagaba** al `build_http_request`  
   - Fix: parámetro wired en la firma de la función.

7. **`format_spec_error` tenía catch-all inalcanzable** post-merge  
   - Fix: eliminado el brazo `other =>`.

8. **`system_message` demasiado específico** (enumeraba tools)  
   - Fix: `system_message` solo define el rol del agente; las instrucciones de uso van en las descripciones de `SubToolDefinition`.

9. **Patrón conversacional incorrecto** (múltiples `llm_call` nodes)  
   - Fix: UN solo `llm_call` node; se reinvoca el grafo con el mismo `--agent-session-id` cambiando `nodes.<input>.config.default` en el JSON.

10. **fmt drift post-merge** (11 archivos)  
    - Fix: `cargo fmt --all`.

### Archivos clave modificados

```
src/libs/colmena/src/web/application/api_spec_use_case.rs   ← resolve_refs(), threshold 0.1, dual-key cache
src/libs/colmena/src/web/domain/api_spec_port.rs            ← components_schemas field en ParsedSpec
src/libs/colmena/src/web/infrastructure/openapi_adapter.rs  ← popula components_schemas
src/libs/colmena/src/dag_engine/infrastructure/nodes/api_explorer.rs  ← params opcional, errores mejorados
src/libs/colmena/src/llm/domain/tools.rs                    ← items field en ParameterProperty
docs/developer_guide/25_web_nodes.md                        ← $ref resolution, HubSpot demo, uso conversacional
docs/agent_context/node_ports_reference.md                  ← params opcional, $ref behavior
docs/node_configurations.json                               ← params: required: false
```

### Grafos de demo creados

```
tests/graphs/web/api_explorer_petstore.json              ← Petstore 3.0 con Gemini
tests/graphs/web/api_explorer_amadeus_swagger2.json      ← Amadeus Swagger 2.0 con Gemini
tests/graphs/web/api_explorer_hubspot_conversation.json  ← Multi-turn HubSpot con Postgres memory
```

---

## Convenciones críticas del proyecto

> Estas reglas salvaron mucho tiempo — **léelas antes de escribir código o grafos**.

### Cargo / Rust

```bash
# Nombre del crate (NO "colmena")
cargo test -p colmena_dag_engine --lib <modulo>
cargo run --bin dag_engine -- run tests/graphs/web/api_explorer_petstore.json

# Toolchain: pinado a 1.95.0 via rust-toolchain.toml
# warnings = "deny" en Cargo.toml — cualquier warning falla el build
```

### Grafos JSON — reglas de oro

1. **Provider string**: usar `"google"` (NO `"gemini"`)  
2. **Memoria**: siempre `${DATABASE_URL}` (Postgres) + Gemini 2.5 Flash como defaults del demo  
3. **Multi-turn**: UN `llm_call` node, reutilizar el grafo cambiando `nodes.<input>.config.default` en JSON para cada turno  
4. **--answer flag**: es para `suspend`/`secure_suspend` QA format, NO para cambiar el input de `llm_call`  
5. **--agent-session-id**: pasar siempre un ID estable para flujos multi-turn/memoria  
6. **system_message**: solo define el ROL del agente, nunca enumera tools  
7. **`api_explorer`**: es SOLO informativo (describe endpoints, construye config); NO ejecuta HTTP — ese es trabajo de `http_request`  

```bash
# Patrón canónico multi-turn
cargo run --bin dag_engine -- run graph.json --agent-session-id agent_demo_001
# (editar nodes.<input>.config.default en graph.json para el siguiente turno)
cargo run --bin dag_engine -- run graph.json --agent-session-id agent_demo_001
```

### Variables de entorno

```bash
source .env   # tiene OPENAI_API_KEY, ANTHROPIC_API_KEY, GEMINI_API_KEY, DATABASE_URL
# NO commitear ni imprimir los valores
```

### agent_session_id válido para pruebas locales

```
cmox2c4ba000n01s66tygjo3d   ← IDs frescos violan FK constraint en llm_node_history
```

---

## Plan B — Siguiente trabajo (browser node)

### Spec y plan

```
docs/superpowers/specs/2026-04-23-web-nodes-b-browser-design.md   ← spec completo
docs/superpowers/plans/2026-04-23-web-nodes-b-browser.md          ← plan de implementación
```

### Resumen del browser node

- **Toolkit node** (`ToolkitNode` trait) que expone sub-tools para controlar un navegador headless  
- **Backend**: Browserless container (WebSocket CDP) + `chromiumoxide` crate en Rust  
- **Sessions**: `SessionRegistry<Arc<BrowserSession>>` keyed by `conversation_id` (mismo patrón que api_explorer)  
- **Sub-tools expuestos**: `navigate`, `click`, `fill`, `fill_secure` (Secure Values para passwords), `extract`, `screenshot`, `wait`, `get_url`, `get_title`, `new_session`, `close_session`  
- **`allow_evaluate`**: flag opcional (default `false`) para activar `evaluate_js` sub-tool  
- **Selector grammar**: CSS por default, más `text=`, `xpath=`, `role=` prefijos  

### Stack técnico para Plan B

```toml
# Cargo.toml — dependencias a añadir
chromiumoxide = "0.7"       # CDP client
tokio-tungstenite = "0.21"  # WebSocket transport (ya puede estar)
```

### Arquitectura de archivos (Plan B)

```
src/libs/colmena/src/web/
├── domain/
│   ├── browser_port.rs          ← BrowserPort trait (nuevo)
│   └── browser_session.rs       ← BrowserSession, SessionHandle (nuevo)
├── application/
│   └── browser_use_case.rs      ← BrowserUseCase (nuevo)
└── infrastructure/
    └── chromiumoxide_adapter.rs  ← impl BrowserPort (nuevo)

src/libs/colmena/src/dag_engine/infrastructure/nodes/
└── browser.rs                   ← BrowserNode : ToolkitNode (nuevo)
```

### Registro en registry.rs

```rust
// Añadir en src/libs/colmena/src/dag_engine/infrastructure/registry.rs
// (mismo patrón que ApiExplorerNode)
```

---

## Arquitectura del módulo web (para contexto)

```
src/libs/colmena/src/web/
├── mod.rs
├── domain/
│   ├── api_spec_port.rs     ← ParsedSpec, ApiSpecPort trait
│   ├── search_port.rs       ← SearchPort trait (Tavily)
│   ├── session.rs           ← SessionRegistry<T> con LRU eviction
│   ├── lifecycle.rs         ← ConversationLifecycleBus
│   └── errors.rs
├── application/
│   ├── api_spec_use_case.rs ← fetch/cache/search/build + resolve_refs()
│   ├── search_use_case.rs   ← Tavily search/fetch use case
│   ├── swagger2_to_oas3.rs  ← Swagger 2.0 → OAS3 normalization
│   └── url_normalizer.rs    ← GitHub blob → raw.githubusercontent.com, etc.
└── infrastructure/
    ├── openapi_adapter.rs   ← impl ApiSpecPort (HTTP fetch + parse)
    └── tavily_adapter.rs    ← impl SearchPort (Tavily API)
```

---

## Cómo retomar el trabajo (checklist rápido)

```bash
# 1. Clonar y entrar
git clone https://github.com/Startti/colmena.git
cd colmena
git checkout develop

# 2. Verificar estado
cargo check
cargo test --lib

# 3. Cargar env vars
source .env

# 4. Probar un grafo (verificar que api_explorer funciona)
cargo run --bin dag_engine -- run tests/graphs/web/api_explorer_petstore.json

# 5. Empezar Plan B
# Leer: docs/superpowers/specs/2026-04-23-web-nodes-b-browser-design.md
# Leer: docs/superpowers/plans/2026-04-23-web-nodes-b-browser.md
```

---

## Contexto adicional

- **Worktree cleanup**: ya limpiado — trabajar directamente en `develop` o crear nuevo worktree en `.worktrees/`  
- **Secure Values**: completamente implementados y testeados (ver `docs/developer_guide/13_security_strategy.md`)  
- **`ToolkitNode` trait**: estudiar `nodes/tavily_client.rs` y `nodes/api_explorer.rs` como reference implementations  
- **Tests post-merge**: 595/595 tests pasaron después del merge del Plan C  
- **Pending tasks generales**: ver `docs/PENDING_TASKS.md`  

---

*Documento generado automáticamente al final de la sesión 2026-05-12. Para detalles de implementación de Plan C, ver el transcript en el historial de git (`git log --oneline` desde el commit `53c4219`).*
