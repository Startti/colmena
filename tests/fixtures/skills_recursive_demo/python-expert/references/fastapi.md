# FastAPI specifics

## Dependency injection essentials

```python
from fastapi import Depends

def get_db():
    db = SessionLocal()
    try:
        yield db
    finally:
        db.close()

@app.get("/items/")
def list_items(db = Depends(get_db)):
    return db.query(Item).all()
```

Dependencies are resolved per-request, cached for the request lifetime if `use_cache=True` (default).

## Pydantic model patterns

- **Request body** = a Pydantic model in the function signature.
- **Response model** = `response_model=MyModel` in the route decorator. Auto-validates and filters output.
- **`Field(..., description="...")`** — surfaces in OpenAPI docs.
- For partial updates: model with `Optional[...]` fields + `exclude_unset=True` in `.dict()`.

## Common gotcha: sync vs async DB drivers

Don't mix `psycopg2` (sync) inside `async def` route handlers — blocks the event loop. Use `asyncpg` or `databases` for async DB access.
