# src/libs/colmena/src/crdt_documents/process_runtime.rs

**Layer:** infrastructure  
**Purpose:** Manages a process-wide singleton for sharing a `CrdtDocumentsRuntime` between the WebSocket server and LLM tool dispatcher, ensuring mutations are visible across both execution contexts.

## Symbols

- `GLOBAL_RUNTIME` (static, private) — thread-safe `OnceCell` holding the single `Arc<CrdtDocumentsRuntime>` for the entire process
- `set_global(rt: Arc<CrdtDocumentsRuntime>) -> Result<(), &'static str>` (fn, pub) — idempotent installation of the process-wide runtime during bootstrap; errors if already set
- `get_global() -> Option<Arc<CrdtDocumentsRuntime>>` (fn, pub) — retrieves a cloned `Arc` to the installed runtime, if available
- `is_installed() -> bool` (fn, pub) — predicate to check if a global runtime has been installed

## File-level notes

- Well-documented module with clear lifecycle semantics: single installation during bootstrap, reference counting prevents premature shutdown, lives for the process lifetime.
- Minimal API surface (3 functions) focused on its single responsibility: singleton management.
- Thread-safe via `OnceCell` (sync, blocking) and `Arc`; no unsafe code.
- No external dependencies beyond `once_cell` and `std`.
- The error message for re-installation is informative and matches the idempotent-with-rejection design.
