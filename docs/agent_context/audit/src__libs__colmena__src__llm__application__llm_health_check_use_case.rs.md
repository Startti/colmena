# src/libs/colmena/src/llm/application/llm_health_check_use_case.rs

**Layer:** application  **Purpose:** Implements a health-check use case that queries the LLM repository abstraction and returns a HealthStatus (Healthy or Unhealthy with reason).

## Symbols

- `LlmHealthCheckUseCase` (struct, pub) — Encapsulates health check business logic with dependency-injected LlmRepository
- `LlmHealthCheckUseCase::new` (fn, pub) — Constructor accepting an Arc-wrapped LlmRepository trait object
- `LlmHealthCheckUseCase::execute` (fn, pub async) — Calls repository.health_check() and maps result to HealthStatus enum  [FLAG: improvement — return type is `Result<HealthStatus, LlmError>` but always returns `Ok(...)`, never `Err(...)`; should be `HealthStatus` directly]
- `LlmHealthCheckUseCase::provider_name` (fn, pub) — Delegates to repository.provider_name() to identify the provider
- `HealthStatus` (enum, pub) — Enum representing health state: Healthy or Unhealthy with reason string
- `HealthStatus::is_healthy` (fn, pub) — Returns true if status is Healthy variant
- `HealthStatus::reason` (fn, pub) — Returns Option<&str> of reason if Unhealthy, None if Healthy
- `tests` (mod, cfg test) — Test module with two async test cases
- `test_health_check_healthy` (fn, test) — Mocks healthy response and verifies is_healthy() and reason() behavior
- `test_health_check_unhealthy` (fn, test) — Mocks network error and verifies is_healthy() returns false and reason contains error message

## File-level notes

- Minimal, focused use case following hexagonal architecture — zero infrastructure dependencies.
- Tests use `mockall` MockLlmRepository to verify both healthy and unhealthy paths.
- Design converts repository errors to HealthStatus::Unhealthy rather than propagating errors, making health checks always succeed with a status result.
