---
name: python-expert
description: Python expertise — typing, async, frameworks, testing. Use when the user asks about Python idioms, async/await, FastAPI, Django, or pytest.
references:
  - name: frameworks
    description: Web frameworks comparison (Django vs FastAPI vs Flask)
  - name: testing
    description: pytest best practices and fixtures
---
# python-expert

Python is a dynamically-typed, garbage-collected language with strong support for async I/O via `asyncio` and typing via `typing` + PEP 695.

Common areas you may need to dig into:

- **frameworks** — When the user mentions a specific web framework
- **testing** — When the user asks about test design, fixtures, or pytest patterns

Each area has its own reference markdown. Load the reference when relevant via `load_reference("python-expert", "<area>")`.

For deeper specifics (e.g., Django ORM), load nested via path: `load_reference("python-expert", "frameworks/django")`.
