---
name: rust_dev
description: Protocol for Rust development in this repository. Use when modifying or creating Rust code in src/libs/colmena.
---

# Rust Development Skill

Detailed instructions for Rust development, ensuring consistency in build processes, testing, and documentation.

## When to use this skill

- Use this when modifying any Rust (`.rs`) files or `Cargo.toml`.
- Use this when creating new Rust modules or libraries.
- Use this when managing dependencies in the Rust workspace.

## How to use it

### 1. Cargo and Build Management
- This is a workspace-based project. The primary Rust library is located in `src/libs/colmena`.
- **Always** run `cargo check` or `cargo build` from the relevant directory to ensure code compiles.
- Use `cargo clippy` and `cargo fmt` to maintain code quality and style.

### 2. Planning and Validation
- **Requirement**: Always create an `implementation_plan.md` before executing any non-trivial code changes.
- **Process**:
    - Describe **what** is being changed and **how** it will be implemented.
    - Show the **exact code blocks** and parts of the file that will be modified.
    - Submit the plan to the user and wait for explicit approval before proceeding to execution.

### 3. Testing
- Every code change should be accompanied by relevant tests.
- Run `cargo test` from the library directory (`src/libs/colmena`) to verify changes.
- Ensure integration tests in the `tests/` directory are also updated if public APIs change.

### 4. Code Standards
- Follow standard Rust idioms and conventions (see "The Rust Programming Language").
- Use `clippy` to identify and fix common mistakes.
- Use triple-slash comments (`///`) for public API documentation.
- Maintain consistency with existing architecture patterns (e.g., error handling using `thiserror` or `anyhow` if applicable).
