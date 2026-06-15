# Auditoría — Bindings de Python (PyO3)

> **Fecha:** 2026-06-14 · **Alcance:** estado de las bindings PyO3 de Colmena para uso desde Python.
> **Foco principal:** motor DAG (`run_dag` / `serve_dag`). · **Audiencia objetivo:** externa / open-source.
> **Estado:** diagnóstico + plan priorizado. **Sin implementación todavía.**

## Veredicto

La funcionalidad core existe y es sólida, pero **toda la documentación de cara al usuario describe una API
que no existe**. Un desarrollador externo falla en la *primera línea* de cada ejemplo publicado
(incluido el README de PyPI). Arreglar la verdad de la documentación es el problema #1, por delante de
cualquier feature nueva.

Fuente de verdad del código auditado:
- [`src/libs/colmena/src/python_bindings/mod.rs`](../../src/libs/colmena/src/python_bindings/mod.rs)
- [`src/libs/colmena/src/python_bindings/crdt_documents.rs`](../../src/libs/colmena/src/python_bindings/crdt_documents.rs)
- [`python/colmena_documents/__init__.py`](../../python/colmena_documents/__init__.py)
- [`python/tests/`](../../python/tests/)

---

## 1. Documentación incorrecta (P0 — rompe al usuario en el primer intento)

| Archivo | Problema | Impacto externo |
|---|---|---|
| [`README_PYPI.md`](../../README_PYPI.md) | `call(model=, temperature=)` kwargs inexistentes; `stream(messages=["str"])` con strings; `for chunk in stream` síncrono (es async-only) | **Crítico** — portada de `pip install colmena-ai` |
| [`docs/examples/python_usage.md`](../examples/python_usage.md) | 926 líneas; API entera ficticia: kwargs sueltos, strings en vez de dicts, sección `files=[...]` de visión/PDF **100% inventada** (grep `files` en bindings = 0 resultados), streaming síncrono | **Crítico** — guía principal de Python |
| [`docs/developer_guide/05a_python_testing.md`](05a_python_testing.md) | Correcto en dicts/async, pero etiqueta como "síncronos" tests de streaming que son async | Menor |
| [`pyproject.toml`](../../pyproject.toml) | versión `0.3.0` vs crate `0.4.0`; clasifica Py3.8–3.12 sin tests que lo respalden | Confianza |

### La verdad del código (API real hoy)

```python
import colmena

llm = colmena.ColmenaLlm()

# call: messages son DICTS {"role","content"}; config va en LlmConfigOptions (NO kwargs sueltos)
opts = colmena.LlmConfigOptions()
opts.model = "gemini-2.5-flash"
opts.temperature = 0.7
resp: str = llm.call(
    messages=[{"role": "user", "content": "Hola"}],
    provider="google",
    options=opts,
)

# stream es ASYNC-ONLY (async for dentro de un event loop)
async def demo():
    stream = llm.stream(messages=[{"role": "user", "content": "Cuenta algo"}], provider="google")
    async for chunk in stream:
        print(chunk, end="")

llm.health_check("google")   # -> bool
llm.get_providers()          # -> list[str]
```

- **No existe** ningún parámetro `files` → toda la sección de visión/PDF en `python_usage.md` es ficción.
- `LlmConfigOptions`, `health_check`, `get_providers` no aparecen en ninguna doc de usuario.

---

## 2. Superficie real sin documentación de usuario (P0/P1 — el foco DAG)

El motor DAG en Python **existe y funciona**, pero no está en la guía de usuario — solo en READMEs sueltos de
`python/tests/`:

| API | Estado doc | Nota |
|---|---|---|
| `run_dag(file_path, resume_id=None, resume_answer=None, inject_payload=None, include_extra_info=False, agent_session_id=None) -> str` | sin doc de usuario | el producto real (estilo ADP); devuelve **JSON string** |
| `serve_dag(file_path, host="0.0.0.0", port=8080) -> None` | sin doc | bloqueante; expone endpoints webhook |
| `validate_graph(graph: dict) -> None` | sin doc | acepta grafo **en memoria** (lanza `DagException` si inválido) |
| `default_registry() -> Registry` | sin doc | sin conexión a DB (inspección) |
| `Registry.node_types() -> list[str]` | sin doc | lista node types registrados |
| `Registry.toolkit_catalog(node_type, config) -> list[dict]` | sin doc | sub-tools de un toolkit |
| submódulo `colmena.documents` (`list_sheets`/`read_sheet`/`add_sheet`/`write_sheet`) + wrapper pandas [`colmena_documents`](../../python/colmena_documents/__init__.py) | sin doc | feature completa invisible |

### Ejemplos rotos

- [`example_run_dag.py`](../../python/tests/example_run_dag.py) usa `tests/power.json` → **no existe**.
- [`example_serve_dag.py`](../../python/tests/example_serve_dag.py) usa `tests/basic_webhook.json` → **no existe**.

Los grafos viven en `tests/graphs/` (basic/, agents/, advanced/, …). Copy-paste de los ejemplos → falla inmediata.

---

## 3. Qué falta por probar

| Superficie | Cobertura actual | Hueco |
|---|---|---|
| `run_dag` (ejecución real) | **0 tests automatizados** — solo scripts-ejemplo sin asserts y con paths rotos | el foco DAG **no tiene red de seguridad** |
| `validate_graph` | indirecto en smokes | sin test directo válido/inválido |
| `serve_dag` | ninguno | arranque + 1 request webhook |
| `resume_id` / `resume_answer` / `inject_payload` / `agent_session_id` | ninguno | suspend→resume desde Python, formato Q/A |
| `call` / `stream` (rutas de error) | parcial | falta key `role`/`content`, provider inválido, `LlmException` |
| `health_check` / `get_providers` | ninguno | trivial, pero está documentado-como-existente |
| `colmena.documents` | roundtrip ✅ | la única superficie bien cubierta |

**Tests existentes y sanos:** `test_mock_streaming`, `test_async_mock_streaming`, `test_complex_scenarios`,
`test_web_nodes`, `test_api_explorer_smoke`, `test_crdt_documents_roundtrip`.

---

## 4. Brechas de diseño para consumo externo

1. **Sin type stubs (`.pyi`) ni `py.typed`** — para open-source es casi obligatorio. Al ser una extensión
   nativa, sin stub el IDE no expone nada y `mypy` no ve la API.
2. **Asimetría `validate_graph(dict)` vs `run_dag(file_path)`** — puedes validar un grafo en memoria pero no
   ejecutarlo sin escribirlo a disco. Idiomático sería `run_dag` aceptando dict o un `run_dag_dict(graph)`.
3. **Tres modelos de runtime tokio distintos** — `call`/`health_check`/`run_dag` crean
   `tokio::runtime::Runtime::new()` **por llamada** (caro); `stream` usa `future_into_py`; `documents`
   exige un runtime ambiente preexistente. Inconsistente y riesgo de rendimiento bajo carga.
4. **`run_dag` devuelve JSON string**, no dict — el usuario hace `json.loads` siempre. Poco idiomático.
5. **Identidad de paquete confusa**: PyPI `colmena-ai` → import `colmena`; además `colmena_documents`
   separado. Falta documentar la relación.
6. **Tool calling / agentes solo vía grafos JSON** — no hay vía programática. Aceptable si el mensaje es
   "Python ejecuta grafos", pero hay que decirlo explícito.

---

## Plan priorizado

> Orden acordado con el usuario (2026-06-14): empezar por **P0 (verdad)** por barato y desbloqueante, luego
> **tests de `run_dag`** porque el foco DAG hoy no tiene cobertura automatizada.

### P0 — Verdad (la doc miente; arréglalo antes que nada) — ✅ COMPLETADO 2026-06-14
1. ✅ Reescritos [`README_PYPI.md`](../../README_PYPI.md) y [`python_usage.md`](../examples/python_usage.md)
   contra la API real: dicts `{role,content}`, `LlmConfigOptions`, `async for` en streaming,
   provider `"google"` (no `"gemini"`), sección `files`/visión **eliminada**. Se añadió sección de
   motor DAG (`run_dag`/`validate_graph`/`serve_dag`/`default_registry`).
2. ✅ Arreglados paths en `example_run_dag.py` → `tests/graphs/basic/power.json`,
   `example_serve_dag.py` → `tests/graphs/basic/power_webhook.json`,
   `example_llm_dag.py` → `tests/graphs/agents/agent_with_tools_gemini.json`.
3. ✅ Versión `pyproject.toml` `0.3.0` → `0.4.0`.

**Verificación E2E:** `tests/graphs/basic/power.json` corre vía CLI y produce `125` (mock_input 5 →
exponential³), coincidiendo con el output documentado en los ejemplos.

### P1 — Documentar y testear el motor DAG (foco) — 🚧 EN CURSO

**🐛 Bug crítico encontrado y arreglado 2026-06-14 (al montar P1):** `colmena.run_dag` (y el
`run_dag` de las bindings de Node) **paniqueaba en todo grafo**. `engine.run_dag` (engine.rs)
enrutaba al stub deprecado `DagRunUseCase::execute()` (`unimplemented!`) en vez de drenar
`execute_stream`. Afectaba: Python `run_dag`, TS `run_dag`, webhook no-SSE y resume no-SSE — el CLI
no, porque ya usaba `execute_stream`. **Fix:** `engine.run_dag` ahora drena `execute_stream` y
devuelve el output del evento terminal `GraphFinish` (y recupera el `agent_session_id` que antes se
ignoraba). Sin cambio de firma → ADP no afectado. Verificado: `cargo check --features python` OK,
782 unit tests del dag_engine OK, E2E `power.json` → 125 desde Python.

**Entorno de dev montado:** pyo3 0.21 (actual) soporta hasta Python 3.12; el `.venv` del root es
3.14 (runtime de `python_script`/pandas) e incompatible para compilar bindings. Se creó
`/Users/danielgarcia/startti/colmena/.venv-dev` sobre **Python 3.12** con `maturin`+`pytest`+
`pytest-asyncio`+`python-dotenv`. `maturin develop` compila las bindings ahí sin tocar pyo3.

**Tests añadidos:** `python/tests/test_run_dag.py` — `run_dag` (output final + archivo inexistente →
`DagException`), `validate_graph` (válido/ inválido), `default_registry().node_types()`. 5/5 pasan.

**Completado en P1 (2026-06-15):**
- ✅ Guía de usuario dedicada [`48_python_dag.md`](48_python_dag.md) (run_dag, validate_graph,
  serve_dag, inject_payload, suspend→resume, registro) + indexada en `DEVELOPER_GUIDE.md`.
- ✅ Tests: `test_run_dag.py` ampliado con `inject_payload` (webhook → 343) y suspend→resume
  (skipif sin `DATABASE_URL`); nuevo `test_serve_dag.py` (smoke webhook out-of-process → 343).
  8 tests en total, todos verdes (verificado E2E con servicios reales).

P1 cubre ahora la superficie DAG documentada y testeada.

**Completado en P2 (2026-06-15):**
- ✅ **Type stubs**: `stubs/colmena/__init__.pyi` + `py.typed`, empaquetados vía
  `[tool.maturin] python-source = "stubs"`. Verificado: el wheel incluye `colmena/__init__.pyi` +
  `colmena/py.typed`, y `mypy` consume las firmas (autocompletado + type-check). Stub clave: `stream`
  devuelve `Awaitable[LlmStream]` (hay que `await`).
- ✅ **`run_dag` acepta dict en memoria** además de path. Refactor de `api::run_dag` (extrae
  `run_dag_from_str`, firma de `run_dag(file_path)` intacta → Node no afectado); binding `run_dag`
  acepta `str | dict`. Tests: dict directo (125) + arg basura → `DagException`. 9/9 en `test_run_dag.py`.
- ✅ **Doc de `colmena.documents`** + wrapper pandas + identidad de paquete
  (`colmena-ai`/`colmena`/`colmena_documents`) en `python_usage.md`.
- ✅ **Bug de doc P0 corregido**: `stream` requiere `await` (devuelve `Future`) — README_PYPI y
  python_usage decían `stream = llm.stream(...)` sin await. Verificado E2E (openai: 8 chunks).

**Hallazgo (P2): `colmena.documents` no es usable desde Python plano** — lanza
`RuntimeError: no tokio runtime available` (a diferencia de `call`/`run_dag` que crean su propio
runtime). Documentado como limitación. Fix = darle runtime propio; encaja en el ítem de runtime
(deferido por decisión del usuario). Por eso `test_crdt_documents_roundtrip` falla sin runtime.

**Deferido (decisión del usuario):** unificar el modelo de runtime tokio (incluye habilitar
`colmena.documents` desde Python plano). Resta también (opcional): `run_dag`/return dict en vez de
JSON string, tool-calling programático.

(plan original abajo)


4. Nueva guía "Colmena DAG desde Python": `run_dag`, `serve_dag`, `validate_graph`, registry,
   resume/`inject_payload`/`agent_session_id`, forma del JSON de retorno y `DagException`.
5. Tests automatizados (pytest) del motor DAG:
   - `run_dag` de un grafo básico real con asserts sobre el output.
   - `validate_graph` válido vs inválido.
   - suspend→resume con formato Q/A canónico.
   - smoke de `serve_dag` (arranque + 1 request).

### P2 — Ergonomía para externos
6. Type stubs `.pyi` + `py.typed` para todo el módulo.
7. `run_dag` que acepte dict en memoria + opción de devolver dict en vez de string.
8. Documentar `colmena.documents` + wrapper pandas, y la relación `colmena-ai` / `colmena` / `colmena_documents`.
9. (Evaluar) unificar el modelo de runtime tokio.

---

## Ejecución

Usar `/python_dev` para los cambios de Python/PyO3 y `/test_graph` para validar grafos usados en los tests.
Recompilar bindings con `maturin develop` antes de correr `pytest python/`.
