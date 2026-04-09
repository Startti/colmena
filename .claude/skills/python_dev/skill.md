---
name: python_dev
description: Protocol for Python development in Colmena. Use when modifying Python files, PyO3 bindings, or Python tests. Includes maturin build context, testing, and documentation integration.
---

# Python Development Skill

## When to Use
- Modifying any Python (`.py`) files
- Creating new Python scripts, tests, or modules
- Modifying PyO3 bindings (Rust side in `python_bindings/mod.rs`)
- Managing Python dependencies or virtual environments

## Project Context

Python code in Colmena falls into two categories:

1. **PyO3 Bindings** (Rust side): `src/libs/colmena/src/python_bindings/mod.rs`
   - Uses `#[pyclass]`, `#[pymethods]`, `pyo3-asyncio` for async
   - Compiles Rust into a native Python module via maturin
   - Key classes: `ColmenaLlm`, `ColmenaDag`, `ColmenaNode`, etc.

2. **Pure Python**: `python/` directory
   - Tests in `python/tests/`
   - Examples and utility scripts

Python is a **consumer** of the Rust core. The compiled module is `colmena` (installed as `colmena-ai` via pip).

### Key Paths
- `src/libs/colmena/src/python_bindings/mod.rs` — Rust-side binding code
- `python/tests/` — Python test files
- `pyproject.toml` — Python package configuration (maturin build backend)
- `docs/PYTHON_USAGE_EXAMPLES.md` — Usage examples for Python users
- `docs/developer_guide/06_estructura_testing_python.md` — Python test organization guide

## Environment Management

- **Always** use the virtual environment at `.venv` in the repo root
- Activate before running scripts or installing packages
- Install dev dependencies: `pip install -e ".[dev]"`

## Build Commands

```bash
maturin develop              # Build PyO3 bindings into .venv (development)
maturin build --release      # Build wheel for distribution
pytest python/tests/         # Run Python tests
```

**Important**: After modifying `python_bindings/mod.rs`, you must run `maturin develop` before Python tests will reflect the changes.

## Planning & Validation

**Requirement**: Always create an `implementation_plan.md` in the repo root (`/home/daniel-garcia4/startti/colmena/implementation_plan.md`) before executing code changes.

1. Describe **what** is being changed and **how**
2. Show the **exact code blocks** and parts of files that will be modified
3. Submit the plan to the user and **wait for explicit approval** before proceeding

## Development Protocols

### Modifying PyO3 Bindings

This is Rust code that generates Python interfaces. Follow both this skill and `/rust_dev`:

1. Modify `src/libs/colmena/src/python_bindings/mod.rs`
2. Use `#[pyclass]` for classes, `#[pymethods]` for methods
3. Handle errors by converting to `PyErr` (use `.map_err(|e| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(e.to_string()))`)
4. For async methods: use `pyo3-asyncio` bridge
5. Run `maturin develop` to rebuild
6. Verify with Python tests

### Writing Python Tests

1. Place tests in `python/tests/`
2. Use `pytest` with `pytest-asyncio` for async tests
3. Use `python-dotenv` for loading environment variables
4. For LLM tests without API calls, use the `MockAdapter` (configure via provider="mock")
5. Follow existing test patterns in `python/tests/`

### Adding Python Examples

1. Add to `docs/PYTHON_USAGE_EXAMPLES.md`
2. Ensure examples are complete, runnable code blocks
3. Include required imports and setup steps

## Testing

Reference: `docs/developer_guide/06_estructura_testing_python.md`

- **Framework**: `pytest` with `pytest-asyncio`
- **Test location**: `python/tests/`
- **Environment**: load `.env` with `python-dotenv` for API keys
- **Mocking**: use `MockAdapter` provider for tests without API consumption
- Every code change should be accompanied by relevant tests
- Run `pytest python/tests/` to verify all tests pass

## Review Checklist

Every code change must include a review addressing:
- **Potential Issues**: side effects, performance impacts, or bugs
- **Affected Scripts**: ALL scripts and modules affected, including dependencies
- **Explanations**: clear "what" and "how" for every significant change

## Documentation (Integrated)

After any code change:

### Code Documentation
- Add/update Google-style docstrings on all modified public functions and classes
- Include type hints (Python 3.8+ compatible)
- Example:
  ```python
  def call(self, prompt: str, system_prompt: Optional[str] = None) -> str:
      """Make a synchronous LLM call.

      Args:
          prompt: The user prompt to send.
          system_prompt: Optional system instructions.

      Returns:
          The model's response text.
      """
  ```

### Project Documentation
- Update `docs/PYTHON_USAGE_EXAMPLES.md` if Python API changes
- Verify `docs/developer_guide/06_estructura_testing_python.md` stays current
- Update `docs/INSTALLATION_GUIDE.md` if setup requirements change
- Check `docs/PENDING_TASKS.md` — does this change resolve any pending item?

## Cross-Binding Coordination

Changes to Python binding interfaces almost always require Rust-side changes:
- If modifying `python_bindings/mod.rs`, also follow the `/rust_dev` skill for the Rust side
- If Rust domain/application APIs change, bindings may need updating
- After binding changes, verify both `cargo test` and `pytest` pass

## Code Standards

- Follow PEP 8 guidelines
- Use `black` for formatting (line-length 88, target Python 3.8)
- Use `isort` for import ordering (profile "black")
- Proper logging and error handling
- Maintain consistency with existing architecture patterns
- Type hints on all function signatures
