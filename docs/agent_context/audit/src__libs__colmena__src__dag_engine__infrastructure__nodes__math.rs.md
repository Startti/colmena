# src/libs/colmena/src/dag_engine/infrastructure/nodes/math.rs

**Layer:** infrastructure  **Purpose:** Provides five basic arithmetic operation nodes (Add, Subtract, Multiply, Divide, Exponential) that implement the `ExecutableNode` trait for numeric computations within the DAG engine.

## Symbols

- `MathError` (enum, private) — error type for arithmetic validation failures (NotANumber, DivisionByZero)
- `MathError::NotANumber` (enum variant, private) — error variant for non-numeric input
- `MathError::DivisionByZero` (enum variant, private) — error variant for division-by-zero attempt
- `get_f64()` (fn, private) — extracts f64 from optional serde_json Value, returns MathError if absent or non-numeric
- `AddNode` (struct, pub) — marker struct implementing ExecutableNode for binary addition (a + b)
- `ExecutableNode impl for AddNode` (impl, pub) — implements execute, default_output, schema for addition
- `SubtractNode` (struct, pub) — marker struct implementing ExecutableNode for binary subtraction (a - b)
- `ExecutableNode impl for SubtractNode` (impl, pub) — implements execute, default_output, schema for subtraction
- `MultiplyNode` (struct, pub) — marker struct implementing ExecutableNode for binary multiplication (a * b)
- `ExecutableNode impl for MultiplyNode` (impl, pub) — implements execute, default_output, schema for multiplication
- `DivideNode` (struct, pub) — marker struct implementing ExecutableNode for binary division (a / b) with zero-check guard
- `ExecutableNode impl for DivideNode` (impl, pub) — implements execute with division-by-zero validation, default_output, schema
- `ExponentialNode` (struct, pub) — marker struct implementing ExecutableNode for exponentiation (input ^ config.exponent)
- `ExecutableNode impl for ExponentialNode` (impl, pub) — implements execute reading base from input and exponent from config, default_input, default_output, schema

## File-level notes

- **Code duplication across binary operators**: AddNode, SubtractNode, MultiplyNode, and DivideNode follow near-identical patterns (same input schema, same error handling, same output wrapping). A procedural macro or code-generation approach could reduce boilerplate, though this is maintainable as-is and trait impls in Rust resist easy macro factoring.
- **Mixed-language comments**: Imports section and MathError variants use Spanish comments; codebase elsewhere is English. Stylistically inconsistent but not functional.
- **Floating-point zero comparison**: Line 109 uses `b == 0.0` for division-by-zero guard. Functionally correct; `is_zero()` would be equally clear.
- **ExponentialNode schema differs**: Takes exponent from `config` rather than `inputs`, making its schema distinct from the binary operators and requiring `default_input()` override. Correctly implemented but visually breaks the pattern.
- **No registration or discovery mechanism in this file**: All nodes are exported as public structs; actual registration in the DAG engine's registry (e.g., `registry.rs`) is not visible here and must be cross-referenced to confirm these nodes are discoverable.
