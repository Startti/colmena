# src/libs/colmena/src/documents/infrastructure/render/mod.rs

**Layer:** infrastructure  **Purpose:** Module barrel-export for document renderers (Excel, HTML, Word formats). Provides a clean public API for the three rendering adapters living in submodules.

## Symbols

- `excel_renderer` (mod, pub) — submodule containing Excel document rendering implementation
- `html_renderer` (mod, pub) — submodule containing HTML document rendering implementation
- `word_renderer` (mod, pub) — submodule containing Word document rendering implementation
- `ExcelRenderer` (type, pub) — re-export of Excel renderer type from excel_renderer module
- `HtmlRenderer` (type, pub) — re-export of HTML renderer type from html_renderer module
- `WordRenderer` (type, pub) — re-export of Word renderer type from word_renderer module

## File-level notes

- Pure barrel-export file with zero logic; all implementation lives in submodules.
- Follows clean hexagonal pattern: infrastructure layer exports concrete renderer types via trait-driven ports.
- No concerns; straightforward module organization.
