# Colmena - AI Agent Orchestration Library

## Project Identity
- Rust-native AI agent orchestration library with Python (PyO3) and TypeScript (napi-rs) bindings
- **Hexagonal Architecture** (Ports & Adapters): domain / application / infrastructure layers
- Version: 0.3.0 (alpha) — Phases 1-6 and 9 complete, Phase 7 (testing) and 8 (docs) pending
- Repository: https://github.com/Startti/colmena

## Key Directories
- `src/libs/colmena/src/` — All Rust source code
  - `llm/` — LLM module: multi-provider abstraction (OpenAI, Anthropic, Gemini)
    - `domain/` — Traits (`LlmRepository`), value objects, errors
    - `application/` — Use cases (`LlmCallUseCase`, `AgentService`)
    - `infrastructure/` — Provider adapters, persistence
  - `dag_engine/` — DAG execution engine (25+ node types)
    - `domain/` — Graph structures, `ExecutableNode` trait
    - `application/` — DAG run orchestration
    - `infrastructure/` — Node implementations in `nodes/`, CLI, REST server
  - `python_bindings/` — PyO3 bindings (`#[pyclass]`, `#[pymethods]`)
  - `node_bindings/` — napi-rs bindings (`#[napi]`, `#[napi(object)]`)
  - `shared/` — Config resolver, service container
- `python/tests/` — Python test scripts
- `tests/` — Rust integration tests
- `tests/graphs/` — JSON DAG test graphs (basic/, agents/, advanced/, memory/, media/)
- `docs/` — Project documentation
  - `dds/` — 5 design documents (architecture, DAG, agents, LLM, RAG)
  - `developer_guide/` — 12 guides (01_architecture through 12_dag_engine_guide)
  - `agent_context/` — nodes_documentation.md, connections_documentation.md

## Build Commands
- **Rust**: `cargo check`, `cargo build`, `cargo test`, `cargo clippy`, `cargo fmt`
- **Python**: `maturin develop` (builds PyO3 bindings into `.venv`)
- **TypeScript**: `npm run build` (napi build with `--features node`)
- **DAG Engine CLI**: `cargo run --bin dag_engine -- --file <path>`
- **Docs**: `cargo doc --no-deps --open`

## Architecture Rules
- Domain layer has **ZERO** infrastructure dependencies
- All external integrations go through traits (ports) defined in domain
- Infrastructure layer implements adapters for those ports
- Use `thiserror` for domain errors, `anyhow` for infrastructure
- All DAG nodes implement the `ExecutableNode` trait
- Node outputs use `{ "output": ... }` convention

## Feature Flags
- `python` — enables PyO3 bindings
- `node` — enables napi-rs bindings
- Default: neither (pure Rust library)

## Testing
- Unit tests: inline `#[cfg(test)]` modules in source files
- Integration tests: `tests/` (Rust), `python/tests/` (Python)
- Mocking: `mockall` crate, `MockAdapter` for LLM tests without API calls
- Test graphs: `tests/graphs/` JSON files

## Conventions
- **Rust**: PascalCase types, snake_case functions, `///` doc comments on all public items
- **Python**: PEP 8, Google-style docstrings
- **Errors**: `Result<T, DomainError>` in domain, `?` propagation
- **Docs language**: Spanish in `docs/`, English in code comments and API docs

## Skills
- `/rust_dev` — Rust development protocol (architecture-aware, includes documentation)
- `/python_dev` — Python development protocol (PyO3/maturin-aware, includes documentation)
- `/typescript_dev` — TypeScript/Node.js development protocol (napi-rs-aware, includes documentation)
- `/ideation` — Expert critic and ideation partner for brainstorming and planning

## Current Status
- See `docs/PENDING_TASKS.md` for detailed task tracking
