# Revisión de CI/CD — Colmena (2026-06-17)

> **Audiencia:** equipo que tomará el mantenimiento del pipeline de CI/CD.
> **Alcance:** los 4 workflows en `.github/workflows/`. Autocontenido — incluye
> rutas, números de línea, comandos y el análisis del fallo de CI del PR
> [#105](https://github.com/Startti/colmena/pull/105).

---

## 1. Resumen ejecutivo

El repo tiene **4 workflows** de GitHub Actions. Funcionan, pero arrastran
deuda técnica que conviene atacar:

- **El más material:** en `ci-develop.yml` todos los pasos de Rust y Node corren
  **7 veces** (una por versión de Python en la matriz), sin caching → ~7× costo/tiempo.
- **El más sutil:** los tests de Python **no gatean** (un `|| echo` se traga los fallos),
  así que tests Python rotos pasan CI en verde.
- **El de publicación:** el job de npm estuvo **roto durante meses** (publicaba versión
  stale y de una sola plataforma) — npm quedó en `0.3.2` mientras PyPI avanzó a `0.3.3`.
  Arreglado en el PR #105.

El **incidente de CI del PR #105** (§4) tuvo dos causas concretas (glob de tests no
soportado en Node 20 + tests de DAG que exigen `DATABASE_URL`), ambas ya corregidas en
el mismo PR.

---

## 2. Inventario de workflows

| Workflow | Trigger | Qué hace | Estado |
|---|---|---|---|
| `validate-commits.yml` | PR a `develop`/`staging`/`main` | Valida Conventional Commits | ✅ OK |
| `ci-develop.yml` | push/PR a `develop` | Rust (fmt/clippy/test) + build/test Python (matriz 3.8–3.14) + build/test Node | ⚠️ deuda (ver §3) |
| `ci-staging.yml` | push a `staging` | Rust checks + build wheels + publish **TestPyPI** (pre-release con timestamp) | ⚠️ sin Node |
| `cd-main.yml` | push a `main` | Bump semántico + tag + build/publish **PyPI** + **npm** + GitHub Release | ⚠️ ver §5 |

### Flujos de publicación (cómo funciona hoy)

**`cd-main.yml`** (release real, en push a `main`):

```
release ─┬─> build-wheels ─┐
         │   sdist ─────────┼─> publish-pypi ─┐
         │                                     ├─> github-release
         └─> build-node ───────> publish-npm ─┘
```

- **`release`** corre `cargo test --verbose`, calcula el bump semántico desde el último
  commit (feat→minor, fix→patch, default→patch), reescribe la versión en `pyproject.toml`
  + `Cargo.toml` + `package.json`, commitea a `main` y crea el tag `vX.Y.Z`.
  Todos los jobs de build hacen `checkout ref: v${new_version}`.
  > No hay loop infinito: el push del bump usa `GITHUB_TOKEN`, y los pushes con ese token
  > **no** disparan nuevos workflows (protección nativa de Actions).
- **PyPI:** `build-wheels` (matriz maturin: linux x86_64 manylinux, macOS x86_64 + aarch64,
  Windows x86_64) + `sdist` → `publish-pypi` (token `PYPI_API_TOKEN`).
- **npm:** `build-node` (matriz de 4 targets, cada uno `napi build --release --target`,
  sube el `.node` como artifact) → `publish-npm` (descarga los 4 `.node`, reconstruye loader
  + facade TS, `npm publish`). El `files` allowlist empaqueta los 4 binarios en un solo
  paquete; el loader `index.js` elige el de la plataforma en runtime.

**`ci-staging.yml`** (pre-release en push a `staging`): versión `X.Y.Z.devTIMESTAMP`,
build wheels, publish a **TestPyPI** (`skip-existing: true`). **Solo Python.**

---

## 3. `ci-develop.yml` — deuda principal

`cargo fmt`, `cargo clippy`, `cargo test --verbose`, el script hexagonal y el
**build+test de Node** (`napi build` debug + `npm test`) viven dentro de la **matriz de 7
versiones de Python** — pero **nada de eso depende de la versión de Python**. Se ejecuta
7× sin caching de `~/.cargo`/`target`/`node_modules`.

**Arreglo recomendado** — separar en 2 jobs:

```yaml
jobs:
  rust-and-node:        # corre UNA vez
    steps: [setup Rust, fmt, clippy, cargo test --verbose, hexagonal,
            setup Node 22, npm ci, npm run build:debug, npm test]
  python:               # matriz solo para lo dependiente de Python
    strategy: { matrix: { python-version: [3.8..3.14] } }
    steps: [setup Rust, setup Python, maturin build, install wheel, pytest]
```

Además: el step `Run Python tests` usa `pytest python/ -v || echo "..."` → **traga todos
los fallos**. Quitar el `|| echo` para que gatee (marcar antes los tests que requieren
red/secretos con `skipif`, igual que Rust usa `#[ignore]`).

---

## 4. Incidente: por qué falló el CI del PR #105

**Síntoma:** los 7 legs de `CI - Develop` fallaron; `validate-commits` pasó. El paso
fallido fue **`Run Node tests`** (los demás legs se cancelaron por `fail-fast`).

**Log del leg 3.10 (el que falló primero):**
```
> COLMENA_LOCAL_STORAGE_PORT=0 node --test 'lib/test/*.js'
Could not find '/home/runner/work/colmena/colmena/lib/test/*.js'
##[error]Process completed with exit code 1.
```

**Causa raíz 1 (la que tumbó CI):** el glob `'lib/test/*.js'` iba **entre comillas
simples** → el shell no lo expande, y **Node 20** (el de CI) **no soporta globs en
`--test`** (llegó en Node 21+). Local pasaba porque el dev corre Node 24/26.

**Causa raíz 2 (latente):** aun con el glob arreglado, `run-dag.test.ts` y
`stream-dag.test.ts` ejecutan el engine real, que exige `DATABASE_URL`
(`EngineConfig::from_env` → "DATABASE_URL must be set to build ColmenaEngine") incluso para
un grafo trivial. Los tests Python equivalentes (`test_run_dag.py`, `test_stream_dag.py`)
fallan igual, pero ahí el `|| echo` lo oculta.

> Nota: los fallos de Python visibles en el log (`GEMINI_API_KEY`, `pandas`/`ulid`
> faltantes, `DATABASE_URL`) **no** tumbaron CI — el `|| echo` los absorbe. Son ruido,
> pero evidencian que la suite Python tampoco gatea (ver §3).

**Fix aplicado en el PR #105** (commit `17310e4`):
1. Gateo `run-dag`/`stream-dag` por `DATABASE_URL` con `{ skip: ... }` de `node:test`
   (espejo de los `#[ignore]` de Rust). Sin DB, se **saltan** en vez de fallar.
2. Glob sin comillas → lo expande el shell (independiente de versión de Node).
3. `setup-node` 20 → **22** en CI y CD (LTS, soporta globs nativos, quita el warning de
   deprecación de Node 20 en runners).

Verificado local simulando CI (`env -u DATABASE_URL npm test`): **13 tests → 10 pass,
3 skipped, 0 fail**.

---

## 5. Tabla priorizada de mejoras

| # | Workflow | Problema | Arreglo | Esfuerzo | Impacto | Estado |
|---|---|---|---|---|---|---|
| 1 | `ci-develop` | Rust+Node corren 7× dentro de la matriz Python | Separar en jobs `rust-and-node` (1×) + `python` (matriz) | M | **Alto** | pendiente |
| 2 | `ci-develop` | `pytest ... \|\| echo` no gatea | Quitar `\|\| echo`; marcar tests env-gated con `skipif` | S | **Alto** | pendiente |
| 3 | `ci-develop`/`cd-main` | Sin caching cargo/target/npm | `Swatinem/rust-cache` + `setup-node cache: npm` | S-M | **Alto** | pendiente |
| 4 | `cd-main` | `release` no gatea sobre build/test Node | Agregar build+test Node al gate (o publish atómico) | M | Medio | pendiente |
| 5 | `cd-main` | Doble `napi build` en `publish-npm` (paso + `prepublishOnly`) | Quitar el `napi build` explícito o `prepublishOnly` | S | Medio | pendiente |
| 6 | `ci-staging` | Sin cobertura ni pre-release de Node | Job Node + `npm publish --tag dev` | M | Medio | pendiente |
| 7 | `cd-main` | Paquete npm ~50MB (4 binarios en 1 tarball) | Migrar a sub-paquetes opcionales por plataforma (`napi prepublish`) | L | Medio | pendiente |
| 8 | repo | Sin escaneo seguridad/deps | `cargo audit` + `npm audit` + Dependabot | S-M | Medio | pendiente |
| 9 | `cd-main` | Sin build musl/Alpine (`require` falla en Alpine) | Agregar target `x86_64-unknown-linux-musl` | M | Bajo-Medio | pendiente |
| 10 | `cd-main` | PyPI con token aunque tiene `id-token: write` sin usar | Migrar a Trusted Publishing (OIDC) | S-M | Bajo | pendiente |
| 11 | `cd-main` | `sed` de versión en `Cargo.toml` raíz es no-op (workspace) | Apuntar al manifest correcto / `workspace.package.version` | S | Bajo | pendiente |
| 12 | `cd-main` | Re-run tras fallo → doble bump / colisión de tag | Calcular versión desde el último tag git, no del archivo | M | Bajo-Medio | pendiente |
| 13 | `ci-staging` | Step "Comment on PR" inalcanzable (trigger solo `push`) | Quitar el step o agregar trigger `pull_request` | S | Bajo | pendiente |
| 14 | `validate-commits` | Rama `else` (push) inalcanzable (trigger solo `pull_request`) | Quitar la rama muerta | S | Bajo | pendiente |

**Esfuerzo:** S = <30 min · M = ~1-3 h · L = medio día+.
**Origen:** #5 y #7 nacen del PR #105 (bindings TS); el resto son pre-existentes.

### Ya resuelto en el PR #105 (contexto)
- `publish-npm` multiplataforma + versión correcta + `files` allowlist (antes npm quedaba
  1 versión atrás de PyPI por `checkout` sin `ref`).
- Binario `.node` destrackeado de git (era un artefacto stale de 2026-03; un rebuild debug
  de 134MB bloqueó un push por el límite de 100MB de GitHub). Ahora `*.node` está gitignoreado.
- Fix del incidente de CI (§4).

---

## 6. Orden recomendado

1. **Quick wins de alto impacto:** #1 (separar matriz), #3 (caching), #2 (gate pytest —
   ojo con `skipif` antes). Bajan el costo de CI y cierran un hueco de calidad real.
2. **Integridad de release:** #4 (gate Node en `release`), #5 (doble build).
3. **Cobertura y empaquetado:** #6 (Node en staging), #7 (sub-paquetes npm), #9 (musl).
4. **Higiene:** #8 (seguridad/deps), #10 (OIDC), #11, #12, #13, #14.

---

## 7. Referencias

- Workflows: `.github/workflows/{validate-commits,ci-develop,ci-staging,cd-main}.yml`
- Convención de tests con DB/red: `#[ignore = "requires X"]` en Rust
  (`docs/developer_guide/05_testing.md`); espejo en Node con `{ skip }` de `node:test`.
- PR de bindings TS que motivó esta revisión:
  [Startti/colmena#105](https://github.com/Startti/colmena/pull/105).
