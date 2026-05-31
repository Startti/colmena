---
references:
  - name: django
    description: Django specifics — ORM, middleware, admin
  - name: fastapi
    description: FastAPI specifics — dependency injection, Pydantic models
---
# Python web frameworks

Three dominant choices in 2026:

| Framework | Best for | Async |
|---|---|---|
| Django | Batteries-included monolith, admin panel, ORM | Partial (4.2+) |
| FastAPI | Type-driven REST APIs, OpenAPI auto-gen | Native |
| Flask | Minimal microservices, full control | Via Quart |

For framework-specific deep dives, navigate to:

- `load_reference("python-expert", "frameworks/django")` — Django ORM details
- `load_reference("python-expert", "frameworks/fastapi")` — FastAPI patterns
