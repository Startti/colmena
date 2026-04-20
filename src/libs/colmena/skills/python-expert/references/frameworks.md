# Python Web Frameworks Reference

## Django
- Prefer class-based views for anything beyond trivial CRUD.
- Use `select_related` / `prefetch_related` to avoid N+1 queries.
- `Model.objects.bulk_create` / `bulk_update` for high-volume writes.
- Put business logic in service modules, not views or models.
- Migrations are source code: review and commit them.

## FastAPI
- Use Pydantic models for request and response schemas.
- Dependency injection via `Depends(...)` for auth, database sessions, and config.
- For long-running work, return 202 and run via background tasks or a proper queue.
- `response_model_exclude_unset=True` when returning partial objects.
- Test with `httpx.AsyncClient` + `LifespanManager`.

## Flask
- Use `Flask` application factories (`create_app()`) plus blueprints for modularity.
- Prefer `flask-sqlalchemy` only for small apps; direct SQLAlchemy for anything serious.
- Use `Flask-Pydantic` or `marshmallow` for validation; Flask does not validate input natively.
- Avoid global state — it breaks when you add workers.

## Choosing between them
- Django: full stack, admin built in, mature ORM, many features out of the box. Good for content-heavy apps.
- FastAPI: async-first, modern, best for APIs. Pick this for new services.
- Flask: minimal and flexible. Pick when you want to wire everything yourself.
