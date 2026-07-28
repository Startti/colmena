# src/libs/colmena/src/documents/infrastructure/validation/mod.rs

**Layer:** infrastructure  
**Purpose:** Module aggregator for three document validators (Excel, HTML, Word); provides public interface to validation adapters in the infrastructure layer.

## Symbols

- `excel_validator` (mod, pub) — submodule providing Excel document validation adapter
- `html_validator` (mod, pub) — submodule providing HTML document validation adapter
- `word_validator` (mod, pub) — submodule providing Word document validation adapter
- `ExcelValidator` (type, pub use) — re-exported Excel validator type from excel_validator module
- `HtmlValidator` (type, pub use) — re-exported HTML validator type from html_validator module
- `WordValidator` (type, pub use) — re-exported Word validator type from word_validator module

## File-level notes

- Clean module aggregator with consistent re-export pattern (each submodule paired with corresponding pub use)
- No public functions, traits, or implementation blocks defined directly in this file
- No conditional compilation, feature flags, or external dependencies visible
- No documentation comments present (stylistic only, not flagged as issue)
