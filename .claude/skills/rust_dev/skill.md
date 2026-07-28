---
name: rust_dev
description: Protocol for Rust development in Colmena. Use when modifying or creating Rust code (.rs files or Cargo.toml) in src/libs/colmena. Includes architecture context, development protocols, testing, and documentation integration.
---

> **MANDATORY — Colmena work uses Colmena nodes.** Any task in/for Colmena MUST be
> built and verified as a real Colmena graph of registered nodes run through the DAG
> engine — never as a standalone script tested in isolation. If a task needs new
> behavior as a tool/step, implement it as a real `ExecutableNode`, register it in
> `registry.rs`, and exercise it inside a graph E2E — do not deliver a standalone
> library.

# Rust Development Skill

## When to Use
- Modifying any Rust (`.rs`) files or `Cargo.toml`
- Creating new Rust modules, nodes, or providers
- Managing dependencies in the Rust workspace

## Project Architecture

Colmena follows **Hexagonal Architecture** (Ports & Adapters) with strict layer separation:

```
Domain (traits, value objects, errors)
   ↓ depends on nothing external
Application (use cases, orchestration)
   ↓ depends on domain only
Infrastructure (adapters, implementations)
   ↓ implements domain traits
```

**Critical rule**: Domain layer has ZERO infrastructure dependencies. All external integrations go through traits (ports) defined in domain.

### Key Paths
- `src/libs/colmena/src/llm/` — LLM module
  - `domain/` — `LlmRepository` trait, `LlmRequest`, `LlmResponse`, `ToolDefinition`, `ToolCall`
  - `application/` — `LlmCallUseCase`, `LlmStreamUseCase`, `AgentService` (ReAct loop)
  - `infrastructure/` — `OpenAiAdapter`, `AnthropicAdapter`, `GeminiAdapter`, `MockAdapter`
- `src/libs/colmena/src/dag_engine/` — DAG engine
  - `domain/` — `ExecutableNode` trait, graph structures
  - `application/` — DAG run orchestration
  - `infrastructure/nodes/` — 25+ node implementations (math, HTTP, LLM, Python, etc.)
- `src/libs/colmena/src/python_bindings/` — PyO3 bindings
- `src/libs/colmena/src/node_bindings/` — napi-rs Node.js bindings
- `src/libs/colmena/src/shared/` — Config resolver, service container

## Build & Quality

```bash
cargo check                    # Fast compilation check
cargo build                    # Full build
cargo clippy                   # Lint — fix all warnings
cargo fmt                      # Format — run before every commit
cargo test                     # Run all tests
cargo doc --no-deps --open     # Verify generated documentation
```

Run from repo root or `src/libs/colmena/`.

## Planning & Validation

**Requirement**: Always create an `implementation_plan.md` in the repo root (`/home/daniel-garcia4/startti/colmena/implementation_plan.md`) before executing non-trivial code changes.

1. Describe **what** is being changed and **how**
2. Show the **exact code blocks** and parts of files that will be modified
3. Submit the plan to the user and **wait for explicit approval** before proceeding

## Development Protocols

### Adding/Modifying DAG Nodes

Reference: `docs/developer_guide/12_dag_engine_guide.md`

1. Create a new file in `src/libs/colmena/src/dag_engine/infrastructure/nodes/`
2. Implement the `ExecutableNode` trait:
   - `node_type()` — returns the node type string
   - `description()` — human-readable description (used by tool discovery)
   - `execute()` — core logic, returns `NodeOutput`
3. Return outputs in `{ "output": ... }` convention
4. Register the node in the `NodeRegistry`
5. Add the node type to `docs/agent_context/nodes_documentation.md`

### Adding/Modifying LLM Providers

Reference: `docs/developer_guide/04_adding_providers.md`

1. Implement the `LlmRepository` trait in `llm/infrastructure/`
2. Handle: basic calls, streaming, tool calling, vision/documents
3. Add to `LlmProviderFactory`
4. Add integration tests with `MockAdapter` fallback
5. Update `docs/dds/MODULO_LLM_DISEÑO.md`

### Modifying Domain Layer

- Never add infrastructure dependencies (no HTTP clients, no DB drivers, no external crates)
- Use `thiserror` for domain error enums with specific variants
- Changes here affect ALL providers and adapters — verify with full `cargo test`

### Working with Bindings

Rust changes to public APIs may affect:
- **Python bindings**: `python_bindings/mod.rs` — uses `#[pyclass]`, `#[pymethods]`
- **Node.js bindings**: `node_bindings/mod.rs` — uses `#[napi]`, `#[napi(object)]`

When changing public interfaces, check if bindings need updating. Coordinate with `/python_dev` or `/typescript_dev` skills as needed.

## Testing

Reference: `docs/developer_guide/05_testing.md`

- **Unit tests**: Inline `#[cfg(test)]` modules in the same file as the code
- **Integration tests**: `tests/` directory for cross-module tests
- **Test graphs**: `tests/graphs/` for DAG execution tests
- **Mocking patterns**:
  - `mockall` crate for trait mocking
  - `MockAdapter` for LLM tests without API calls
  - `wiremock` for HTTP endpoint mocking
- Every code change should be accompanied by relevant tests
- Run `cargo test` to verify all tests pass before submitting

## Documentation (Integrated)

After any code change:

### Code Documentation
- Add/update `///` triple-slash doc comments on all modified public APIs
- Include at least one usage example in doc comments for new public functions
- Run `cargo doc --no-deps` to verify generated docs render correctly

### Project Documentation
- Run `git diff` to identify which docs in `docs/` are affected
- Update relevant files:
  - Architecture changes → `docs/dds/ARQUITECTURA_HEXAGONAL_GUIA.md`
  - LLM module → `docs/dds/MODULO_LLM_DISEÑO.md`
  - DAG engine → `docs/dds/DAG_ENGINE_DISEÑO.md`, `docs/developer_guide/12_dag_engine_guide.md`
  - Agent/tool changes → `docs/dds/DISEÑO_AGENTES_Y_TOOLS.md`
  - New nodes → `docs/agent_context/nodes_documentation.md`
  - New connections → `docs/agent_context/connections_documentation.md`
- Check `docs/PENDING_TASKS.md` — does this change resolve any pending item?

## Design Documents (Read Before Major Work)

Before starting work on a major subsystem, read the relevant design doc:
- LLM work: `docs/dds/MODULO_LLM_DISEÑO.md`
- DAG work: `docs/dds/DAG_ENGINE_DISEÑO.md`
- Agent/Tool work: `docs/dds/DISEÑO_AGENTES_Y_TOOLS.md`
- Architecture questions: `docs/dds/ARQUITECTURA_HEXAGONAL_GUIA.md`
- RAG: `docs/dds/RAG_DISEÑO.md`

## Code Standards

- Follow standard Rust idioms (The Rust Programming Language)
- `clippy` warnings must be resolved — do not allow `#[allow(clippy::...)]` without justification
- Error handling: `thiserror` for domain errors, `anyhow` only at infrastructure boundaries
- Naming: PascalCase for types/traits, snake_case for functions/variables, SCREAMING_SNAKE for constants
- Use `async/await` with Tokio runtime for all async operations
- Prefer `impl Trait` over `dyn Trait` where possible for performance
- Maintain consistency with existing patterns — check neighboring files before introducing new patterns
