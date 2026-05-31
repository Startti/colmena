# pytest best practices

## Fixtures, not setUp/tearDown

```python
import pytest

@pytest.fixture
def db():
    conn = create_connection()
    yield conn
    conn.close()

def test_users(db):
    assert db.query("SELECT 1").scalar() == 1
```

## Parametrize for matrix coverage

```python
@pytest.mark.parametrize("a,b,expected", [
    (1, 2, 3),
    (0, 0, 0),
    (-1, 1, 0),
])
def test_add(a, b, expected):
    assert a + b == expected
```

## Tips

- `conftest.py` shares fixtures across files in a directory (no import).
- `pytest -x` stops at first failure.
- `pytest -k "expr"` filters by test name pattern.
- `pytest --pdb` drops into debugger on failure.
- For async tests use `pytest-asyncio` and mark with `@pytest.mark.asyncio`.
