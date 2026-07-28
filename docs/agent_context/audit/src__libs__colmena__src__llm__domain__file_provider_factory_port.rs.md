# src/libs/colmena/src/llm/domain/file_provider_factory_port.rs

**Layer:** domain  
**Purpose:** Defines the `FileProviderFactoryPort` trait, a hexagonal architecture port that abstracts factory creation of `FileProviderRepository` instances based on provider kind and API key, decoupling use cases from concrete infrastructure implementations.

## Symbols

- `FileProviderFactoryPort` (trait, pub) — Port trait that resolves `(ProviderKind, api_key)` → `Arc<dyn FileProviderRepository>`, allowing dependency injection of test stubs without importing infrastructure code.
- `FileProviderFactoryPort::build` (method, pub) — Factory method that constructs a file provider repository for a given provider; returns `LlmError::ProviderLimitation` if the provider does not support Files API (e.g., Mock).

## File-level notes

- Very focused and minimal port definition, exemplifying clean hexagonal architecture.
- Correctly marked `Send + Sync` for async compatibility.
- Comprehensive error documentation for the `ProviderLimitation` case.
- No imports of infrastructure code; domain stays isolated.
- Well-written Spanish documentation consistent with project style.
