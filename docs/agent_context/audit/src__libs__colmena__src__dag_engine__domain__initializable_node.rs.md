# src/libs/colmena/src/dag_engine/domain/initializable_node.rs

**Layer:** domain  
**Purpose:** Defines the optional initialization trait for DAG nodes requiring one-time setup before execution. Nodes implementing this trait have `initialize()` called once per DAG run before their first `execute()` to create connection pools, load metadata, or perform expensive setup.

## Symbols

- `InitContext` (struct, pub) — Value object returned by node initialization, carries optional description_supplement text for tool enrichment
- `InitContext::description_supplement` (field, pub) — Optional string to append to the tool's LLM-facing description (schema info, available functions, etc.)
- `InitializableNode` (trait, pub) — Async trait marking nodes that need one-time initialization before execution; Send + Sync bound
- `InitializableNode::initialize` (method, async pub) — Called once before first execute() in a DAG run; takes static config, returns InitContext or error

## File-level notes

- Well-documented module with clear separation of concerns (trait + context value object)
- Zero infrastructure dependencies; pure domain contract
- Error handling uses `Box<dyn StdError + Send + Sync>` (idiomatic for trait objects)
- No symbols show usage patterns within this file; this is a definition-only module (coupling measured via blast-radius audit elsewhere)
