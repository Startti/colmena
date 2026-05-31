# Django specifics

## ORM key facts

- **Lazy QuerySets** — `Model.objects.filter(...)` doesn't execute until iterated. Chain freely.
- **`select_related` vs `prefetch_related`** — FK forward = `select_related` (JOIN); M2M/reverse FK = `prefetch_related` (extra query).
- **`.only()` / `.defer()`** — column-level projection. Use sparingly; deferred columns trigger a DB hit on access.
- **`F` expressions** — atomic field operations without race conditions: `Model.objects.update(count=F('count') + 1)`.
- **`Q` objects** — for complex AND/OR queries: `.filter(Q(x=1) | Q(y=2))`.

## Admin gotchas

- `list_display` requires properties or callables for non-model attributes.
- `inlines` with M2M requires `through_fields` if the through model has 3+ FKs.

## Middleware order matters

```python
MIDDLEWARE = [
    'django.middleware.security.SecurityMiddleware',  # FIRST
    'django.contrib.sessions.middleware.SessionMiddleware',
    'django.middleware.common.CommonMiddleware',
    'django.middleware.csrf.CsrfViewMiddleware',
    'django.contrib.auth.middleware.AuthenticationMiddleware',
    # ... your own here ...
    'django.middleware.clickjacking.XFrameOptionsMiddleware',  # LAST
]
```

Auth must run AFTER sessions; CSRF before any view middleware that reads POST.
