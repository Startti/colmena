# src/libs/colmena/src/web/infrastructure/mod.rs

**Layer:** infrastructure  
**Purpose:** Module declaring and re-exporting adapter implementations (openapi_adapter, tavily_adapter) for web-toolkit ports.

## Symbols

- `openapi_adapter` (mod, public) — Submodule containing OpenAPI adapter implementation
- `tavily_adapter` (mod, public) — Submodule containing Tavily search adapter implementation
- `OpenApiAdapter` (re-export, public) — Main type exported from openapi_adapter module
- `OpenApiAdapterConfig` (re-export, public) — Configuration type for OpenAPI adapter
- `TavilyAdapter` (re-export, public) — Main type exported from tavily_adapter module

## File-level notes

- Minimal module file following standard Rust re-export pattern
- All symbols are clean public re-exports; no unused items
- No complexity, dead code, or unfinished sections
