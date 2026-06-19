# Config de modelos baratos (`cheap_models.yaml`) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Centralizar el modelo barato/rápido por provider en un archivo único editable (`cheap_models.yaml`), con override por env var, expuesto vía una función `cheap_model_for(provider)`.

**Architecture:** Es la **Fase 2** del spec `docs/superpowers/specs/2026-06-18-conversation-semantic-summary-design.md`. Archivo YAML embebido con `include_str!` (mismo patrón que `text/`), parseado a `HashMap<String,String>` en un `OnceLock`. Resolución: env `COLMENA_CHEAP_MODEL_<PROVIDER>` → YAML → default del provider. Es infraestructura standalone que consumirá la Fase 4 (summarizer) y, opcionalmente, el summarizer de attachments.

**Tech Stack:** Rust, `serde_yaml`, `std::sync::OnceLock`, `ProviderKind` (`src/libs/colmena/src/llm/domain/llm_provider.rs`).

---

## File Structure

- `src/libs/colmena/text/config/cheap_models.yaml` — **nuevo**: el mapa provider→modelo barato (fuente de verdad editable).
- `src/libs/colmena/src/llm/infrastructure/cheap_models.rs` — **nuevo**: loader + resolución (`cheap_model_for`) + tests.
- `src/libs/colmena/src/llm/infrastructure/mod.rs` — **modificar**: registrar el módulo `cheap_models` y re-exportar `cheap_model_for`.

---

### Task 1: Archivo YAML de modelos baratos

**Files:**
- Create: `src/libs/colmena/text/config/cheap_models.yaml`

- [ ] **Step 1: Crear el YAML**

```yaml
# Modelo barato/rápido por provider para tareas internas de resumen
# (compactación de historial, catálogo de attachments).
# Editá acá; override en runtime con COLMENA_CHEAP_MODEL_<PROVIDER>
# (p.ej. COLMENA_CHEAP_MODEL_GOOGLE=gemini-2.5-flash).
# Regla de proyecto: gemini-2.5-flash, NUNCA gemini-1.5-flash.
openai: gpt-4o-mini
google: gemini-2.5-flash
anthropic: claude-haiku-4-5-20251001
```

- [ ] **Step 2: Commit**

```bash
git add src/libs/colmena/text/config/cheap_models.yaml
git commit -m "feat(memory): add cheap_models.yaml provider->model registry"
```

---

### Task 2: Loader + resolución `cheap_model_for`

**Files:**
- Create: `src/libs/colmena/src/llm/infrastructure/cheap_models.rs`
- Test: mismo archivo (módulo `#[cfg(test)]` inline)

- [ ] **Step 1: Escribir el módulo con la lógica de resolución**

```rust
//! Registro de modelos baratos por provider para tareas internas de resumen.
//!
//! Fuente de verdad: `text/config/cheap_models.yaml` (embebido en compile-time).
//! Resolución (mayor a menor prioridad):
//!   1. Env `COLMENA_CHEAP_MODEL_<PROVIDER>` (runtime, sin rebuild).
//!   2. El YAML embebido.
//!   3. El modelo default del provider (`ProviderKind::default_model`).
//!
//! Ver `docs/superpowers/specs/2026-06-18-conversation-semantic-summary-design.md` §5.

use crate::llm::domain::ProviderKind;
use std::collections::HashMap;
use std::sync::OnceLock;

const CHEAP_MODELS_YAML: &str = include_str!("../../../text/config/cheap_models.yaml");

static CHEAP_MODELS: OnceLock<HashMap<String, String>> = OnceLock::new();

fn registry() -> &'static HashMap<String, String> {
    CHEAP_MODELS.get_or_init(|| {
        serde_yaml::from_str(CHEAP_MODELS_YAML)
            .unwrap_or_else(|e| panic!("text/config/cheap_models.yaml malformed: {e}"))
    })
}

/// Resuelve el modelo barato para un provider, aplicando la cadena env → yaml → default.
pub fn cheap_model_for(provider: ProviderKind) -> String {
    let key = provider.to_string(); // "openai" | "google" | "anthropic" | ...
    let env_var = format!("COLMENA_CHEAP_MODEL_{}", key.to_uppercase());
    if let Ok(v) = std::env::var(&env_var) {
        let v = v.trim().to_string();
        if !v.is_empty() {
            return v;
        }
    }
    if let Some(m) = registry().get(&key) {
        return m.clone();
    }
    // Último recurso: el default del provider (no necesariamente barato, pero válido).
    provider.default_model().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yaml_has_the_three_real_providers() {
        let r = registry();
        assert_eq!(r.get("google").map(|s| s.as_str()), Some("gemini-2.5-flash"));
        assert_eq!(r.get("openai").map(|s| s.as_str()), Some("gpt-4o-mini"));
        assert!(r.contains_key("anthropic"));
    }

    #[test]
    fn yaml_default_when_no_env() {
        // Sin env var para google → cae al YAML.
        std::env::remove_var("COLMENA_CHEAP_MODEL_GOOGLE");
        assert_eq!(cheap_model_for(ProviderKind::Google), "gemini-2.5-flash");
    }

    #[test]
    fn env_override_wins() {
        std::env::set_var("COLMENA_CHEAP_MODEL_OPENAI", "gpt-4o-mini-test-override");
        assert_eq!(
            cheap_model_for(ProviderKind::OpenAi),
            "gpt-4o-mini-test-override"
        );
        std::env::remove_var("COLMENA_CHEAP_MODEL_OPENAI");
    }

    #[test]
    fn never_returns_gemini_1_5() {
        // Guard de la regla de proyecto.
        std::env::remove_var("COLMENA_CHEAP_MODEL_GOOGLE");
        assert_ne!(cheap_model_for(ProviderKind::Google), "gemini-1.5-flash");
    }
}
```

> **Nota:** verificar que `ProviderKind` tenga un método público que devuelva su modelo
> default (en `llm_provider.rs` existe `default_model` — `ProviderKind::OpenAi => "gpt-4o"`).
> Si el nombre real difiere, ajustar la llamada en `cheap_model_for`.

- [ ] **Step 2: Registrar el módulo en `mod.rs`**

En `src/libs/colmena/src/llm/infrastructure/mod.rs`, agregar:

```rust
pub mod cheap_models;
pub use cheap_models::cheap_model_for;
```

- [ ] **Step 3: Correr los tests y verificar que pasan**

Run: `cargo test --lib cheap_models -- --test-threads=1`
Expected: PASS. (`--test-threads=1` porque los tests mutan env vars de proceso y no deben
pisarse entre sí.)

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/llm/infrastructure/cheap_models.rs src/libs/colmena/src/llm/infrastructure/mod.rs
git commit -m "feat(memory): cheap_model_for resolution (env > yaml > provider default)"
```

---

### Task 3: Verificación de la fase

**Files:** ninguno

- [ ] **Step 1: Build + clippy + fmt**

Run:
```bash
cargo test --lib cheap_models -- --test-threads=1 && \
cargo clippy -p colmena_dag_engine --all-targets -- -D warnings && \
cargo fmt --check
```
Expected: PASS / sin warnings / sin diffs.

---

## Self-Review

- **Spec coverage:** cubre §5 del spec (config de modelos baratos + cadena env → yaml →
  default). El override **por nodo** (`summary_model`) se aplica en el call site de la
  Fase 4, no acá — esta fase expone `cheap_model_for(provider)` como base.
- **Placeholder scan:** sin TODOs; el único condicional ("si el nombre real de
  `default_model` difiere") es una verificación de 1 línea con el valor esperado provisto.
- **Type consistency:** `cheap_model_for(provider: ProviderKind) -> String` se usa igual en
  tests y será el contrato que consume la Fase 4. La key del YAML/env es `provider.to_string()`
  (`"openai"/"google"/"anthropic"`), consistente con `Display` de `ProviderKind`.
