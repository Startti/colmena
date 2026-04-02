---
name: typescript_dev
description: Protocol for TypeScript/Node.js development in Colmena. Use when modifying napi-rs bindings, Node.js integration, or TypeScript types. Includes build context, testing, and documentation integration.
---

# TypeScript Development Skill

## When to Use
- Modifying napi-rs bindings in `node_bindings/mod.rs`
- Working with `package.json`, `index.js`, or `index.d.ts`
- Creating or modifying TypeScript/Node.js tests or examples
- Managing npm dependencies or build configuration

## Project Context

TypeScript/Node.js bindings in Colmena use **napi-rs** to expose Rust functionality as a native Node.js module.

- **Published package**: `colmena-ai` (npm)
- **Binding source**: `src/libs/colmena/src/node_bindings/mod.rs`
- **Generated outputs**: `index.js` (native module loader) + `index.d.ts` (TypeScript types)
- **Build system**: `@napi-rs/cli` v2.18+
- **Feature flag**: `--features node` (enables `node_bindings` module in Rust)

TypeScript is a **consumer** of the Rust core, similar to Python bindings.

### Key Paths
- `src/libs/colmena/src/node_bindings/mod.rs` — Rust-side napi binding code
- `package.json` — npm package configuration, build scripts, platform targets
- `index.js` — Generated native module loader (do not edit manually)
- `index.d.ts` — Generated TypeScript type definitions (do not edit manually)

## Build Commands

```bash
npm run build              # Release build (napi build --platform --release --features node)
npm run build:debug        # Debug build (faster, for development)
```

**Important**: After modifying `node_bindings/mod.rs`, you must run `npm run build` before Node.js will reflect the changes. The generated `index.js` and `index.d.ts` are auto-generated — do not edit them manually.

## Planning & Validation

**Requirement**: Always create an `implementation_plan.md` before executing non-trivial code changes.

1. Describe **what** is being changed and **how**
2. Show the **exact code blocks** and parts of files that will be modified
3. Submit the plan to the user and **wait for explicit approval** before proceeding

## Development Protocols

### Adding New Bindings

1. Open `src/libs/colmena/src/node_bindings/mod.rs`
2. Define input/output structs with `#[napi(object)]`:
   ```rust
   #[napi(object)]
   pub struct MyInput {
       pub field: String,
   }
   ```
3. Implement functions/methods with `#[napi]`:
   ```rust
   #[napi]
   pub async fn my_function(input: MyInput) -> napi::Result<String> {
       // Implementation
       Ok(result)
   }
   ```
4. Handle errors via `napi::Error`:
   ```rust
   .map_err(|e| napi::Error::from_reason(e.to_string()))?
   ```
5. Run `npm run build` to rebuild
6. Verify generated types in `index.d.ts` match expectations

### Modifying Existing Bindings

1. Check current binding signature in `node_bindings/mod.rs`
2. Modify the Rust code
3. Rebuild with `npm run build`
4. Verify `index.d.ts` — ensure type changes are intentional and backwards-compatible (or document breaking changes)

### Platform Targets

The package supports multiple platforms (defined in `package.json`):
- `x86_64-apple-darwin` (macOS Intel)
- `aarch64-apple-darwin` (macOS Apple Silicon)
- `x86_64-pc-windows-msvc` (Windows)
- `x86_64-unknown-linux-gnu` (Linux)

Cross-compilation is handled by CI. For local development, `npm run build` targets the host platform.

## Testing

- Test Node.js bindings by writing simple scripts that import `colmena-ai`
- Verify TypeScript types compile correctly
- For LLM tests without API calls, configure the mock provider
- Ensure `npm run build` succeeds without errors before submitting

## Documentation (Integrated)

After any code change:

### Code Documentation
- Add JSDoc-style comments to exported functions in `node_bindings/mod.rs` using napi doc attributes
- Verify generated `index.d.ts` has clear type signatures

### Project Documentation
- Update any TypeScript usage examples if API changes
- Update `docs/INSTALLATION_GUIDE.md` if npm setup requirements change
- Check `docs/PENDING_TASKS.md` — does this change resolve any pending item?

## Cross-Binding Coordination

Changes to Node.js binding interfaces almost always require Rust-side changes:
- If modifying `node_bindings/mod.rs`, also follow the `/rust_dev` skill for the Rust side
- If Rust domain/application APIs change, bindings may need updating
- After binding changes, verify both `cargo test` and `npm run build` succeed

## Code Standards

- Rust side: follow all `/rust_dev` standards for the binding code
- TypeScript types: ensure generated `.d.ts` is clean and usable
- Use `napi::Result<T>` for all fallible operations
- Prefer `#[napi(object)]` structs over loose parameters for complex inputs
- Use `String` (not `&str`) for napi function signatures
- Maintain consistency with existing binding patterns in `node_bindings/mod.rs`
