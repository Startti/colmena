# Guía de CI/CD y Versionado Semántico

Esta guía explica el flujo de CI/CD implementado en Colmena, incluyendo versionado automático, workflows de GitHub Actions y configuración de protección de ramas.

## Tabla de Contenidos

- [Visión General del Flujo](#visión-general-del-flujo)
- [Workflows de GitHub Actions](#workflows-de-github-actions)
- [Versionado Semántico Automático](#versionado-semántico-automático)
- [Configuración de Protección de Ramas](#configuración-de-protección-de-ramas)
- [Configuración de Secretos](#configuración-de-secretos)
- [Flujo de Trabajo Recomendado](#flujo-de-trabajo-recomendado)

---

## Visión General del Flujo

El proyecto utiliza un flujo **GitFlow con staging** con tres ramas principales:

```
develop (desarrollo) ──────► staging (pre-producción) ──────► main (producción)
    │                              │                              │
    │                              │                              │
    ├─ CI automático               ├─ CI automático               ├─ CI automático
    ├─ Tests                       ├─ Tests                       ├─ Tests
    └─ Sin publicación             ├─ Pre-release versioning      ├─ Versionado automático
                                   ├─ Publicación a TestPyPI      ├─ Publicación a PyPI
                                   └─ Testing de integración      └─ GitHub Release
```

### Flujo de Desarrollo

1. **Desarrollo en `develop`**:
   - Todos los desarrollos nuevos van a `develop`
   - Se ejecuta CI completo en cada push/PR
   - **No se publica** ningún paquete

2. **Pre-Release en `staging`**:
   - Merge de `develop` → `staging` para testing de integración
   - Se ejecuta CI completo
   - Se genera versión pre-release (ej: `0.1.0.dev20251004120000`)
   - Se publica a **TestPyPI** para validación
   - Ambiente para QA y testing final

3. **Release en `main`**:
   - Merge de `staging` → `main` inicia el proceso de release final
   - Se ejecuta CI completo
   - Se calcula nueva versión automáticamente
   - Se actualiza versión en archivos
   - Se publica a **PyPI** (producción)
   - Se crea GitHub Release

---

## Workflows de GitHub Actions

### 1. CI para Develop (`.github/workflows/ci-develop.yml`)

**Trigger**: Push o Pull Request a `develop`

**Propósito**: Validar que el código funciona correctamente antes de merge

**Pasos**:
1. **Checkout del código**
2. **Setup de Rust y Python** (matriz con Python 3.8-3.12)
3. **Cache de dependencias de Rust**
4. **Instalación de maturin**
5. **Verificación de formato**: `cargo fmt --check`
6. **Análisis de código**: `cargo clippy`
7. **Tests de Rust**: `cargo test`
8. **Build del paquete Python**: `maturin build`
9. **Instalación del paquete**
10. **Tests de Python**: `pytest` (si existen)

**Resultado**: ✅ o ❌ que indica si el código está listo para merge

---

### 2. CI/CD para Staging (`.github/workflows/ci-staging.yml`)

**Trigger**: Push o Pull Request a `staging`

**Propósito**: Validar y publicar pre-releases para testing

**Pasos**:
1. **Checkout completo**
2. **Setup de Rust y Python**
3. **Cache de dependencias**
4. **Instalación de maturin**
5. **Verificación de formato**: `cargo fmt --check`
6. **Análisis de código**: `cargo clippy`
7. **Tests de Rust**: `cargo test`
8. **🏷️ Generación de versión pre-release**:
   - Formato: `X.Y.Z.devTIMESTAMP` (PEP 440 compatible)
   - Ejemplo: `0.1.0.dev20251004120000`
9. **Build de wheels de Python**
10. **📦 Publicación a TestPyPI** (solo en push, no en PR)

**Resultado**: Pre-release publicado en TestPyPI para testing

---

### 3. CD para Main (`.github/workflows/cd-main.yml`)

**Trigger**: Push a `main`

**Propósito**: Validar, versionar y publicar automáticamente a producción

**Pasos**:
1. **Checkout completo** (con historial completo para versionado)
2. **Setup de Rust y Python**
3. **Cache de dependencias**
4. **Instalación de maturin**
5. **Verificación de formato y calidad**
6. **Tests de Rust**
7. **🔢 Versionado Semántico Automático**:
   - Lee el último commit message
   - Determina tipo de bump (MAJOR/MINOR/PATCH)
   - Actualiza `pyproject.toml` y `Cargo.toml`
   - Crea commit de versión
   - Crea tag Git
8. **Build de wheels de Python**
9. **📦 Publicación a PyPI** (producción)
10. **🎉 Creación de GitHub Release**

**Resultado**: Nueva versión publicada en PyPI y GitHub Releases

---

## Versionado Semántico Automático

El versionado sigue [Semantic Versioning 2.0.0](https://semver.org/) y usa **Conventional Commits** para determinar el tipo de bump.

### Formato de Versión

```
MAJOR.MINOR.PATCH
  │     │     │
  │     │     └─── Bug fixes, refactoring
  │     └─────────── Nuevas features (backward compatible)
  └───────────────── Breaking changes
```

### Conventional Commits y Versionado

| Tipo de Commit | Ejemplo | Bump | Versión |
|----------------|---------|------|---------|
| Breaking Change | `feat!: cambio incompatible`<br>`BREAKING CHANGE: ...` | MAJOR | 1.0.0 → **2.0.0** |
| Nueva Feature | `feat: agregar soporte para Gemini`<br>`feature(llm): nuevo provider` | MINOR | 1.0.0 → **1.1.0** |
| Bug Fix | `fix: corregir timeout en OpenAI`<br>`bugfix(api): manejo de errores` | PATCH | 1.0.0 → **1.0.1** |
| Performance | `perf: optimizar llamadas a API` | PATCH | 1.0.0 → **1.0.1** |
| Refactoring | `refactor: simplificar arquitectura` | PATCH | 1.0.0 → **1.0.1** |
| Otros | `chore:`, `docs:`, `test:` | PATCH | 1.0.0 → **1.0.1** |

### Ejemplos de Commits

#### ✅ Commits Correctos

```bash
# Feature nueva (MINOR bump: 1.0.0 → 1.1.0)
git commit -m "feat: add support for Claude 3.5 Sonnet"

# Bug fix (PATCH bump: 1.0.0 → 1.0.1)
git commit -m "fix: resolve timeout issue in streaming responses"

# Breaking change (MAJOR bump: 1.0.0 → 2.0.0)
git commit -m "feat!: change API signature for create_agent"

# Con scope
git commit -m "feat(providers): add Gemini Pro support"

# Breaking change con descripción
git commit -m "refactor: restructure configuration

BREAKING CHANGE: Configuration format has changed from JSON to TOML"
```

#### ❌ Commits Incorrectos

```bash
# Sin tipo convencional (defaultea a PATCH)
git commit -m "Added new feature"

# Typo en el tipo
git commit -m "feature: add something"  # Debería ser "feat:"
```

### Algoritmo de Versionado

El workflow analiza el **último commit message** de `main` para determinar el bump:

```bash
# 1. MAJOR bump (Breaking Changes)
^(feat|feature)(\(.+\))?!:
^BREAKING CHANGE:

# 2. MINOR bump (New Features)
^(feat|feature)(\(.+\))?:

# 3. PATCH bump (Fixes, Performance, Refactoring)
^(fix|bugfix|perf|refactor)(\(.+\))?:

# 4. Default (cualquier otro): PATCH bump
```

---

## Configuración de Protección de Ramas

Para mantener la calidad del código y evitar merges accidentales, debes configurar **Branch Protection Rules** en GitHub.

### Pasos para Configurar

1. Ve a **GitHub** → Tu repositorio
2. **Settings** → **Branches** (en el menú lateral)
3. Haz click en **Add branch protection rule**

### Configuración para `main`

**Branch name pattern**: `main`

**Reglas recomendadas**:

- ✅ **Require a pull request before merging**
  - ✅ **Require approvals**: 1 (o más para equipos grandes)
  - ✅ **Dismiss stale pull request approvals when new commits are pushed**
  - ✅ **Require review from Code Owners** (opcional)

- ✅ **Require status checks to pass before merging**
  - ✅ **Require branches to be up to date before merging**
  - **Status checks que deben pasar**:
    - `Test` (del workflow ci-develop.yml)
    - Todos los jobs de la matriz de Python (3.8, 3.9, 3.10, 3.11, 3.12)

- ✅ **Require conversation resolution before merging**

- ✅ **Require linear history** (evita merge commits)

- ✅ **Do not allow bypassing the above settings**
  - ⚠️ Excepción: Administrators (para emergencias)

- ✅ **Restrict who can push to matching branches**
  - Solo GitHub Actions (para commits automáticos de versión)
  - Tech Leads o Maintainers específicos

**⚠️ IMPORTANTE**: Para que GitHub Actions pueda hacer push de commits de versión:
- En **Settings** → **Actions** → **General**
- En **Workflow permissions**:
  - Selecciona **Read and write permissions**
  - ✅ Marca **Allow GitHub Actions to create and approve pull requests**

### Configuración para `staging`

**Branch name pattern**: `staging`

**Reglas recomendadas**:

- ✅ **Require a pull request before merging**
  - ✅ **Require approvals**: 1

- ✅ **Require status checks to pass before merging**
  - ✅ **Require branches to be up to date before merging**
  - **Status checks que deben pasar**:
    - `Test and Publish to TestPyPI` (del workflow ci-staging.yml)

- ✅ **Require conversation resolution before merging**

- ✅ **Require linear history**

- ✅ **Do not allow bypassing the above settings**

**⚠️ IMPORTANTE**: Para que GitHub Actions pueda publicar a TestPyPI:
- Mismo setup de permisos que `main` (Read and write permissions)

### Configuración para `develop`

**Branch name pattern**: `develop`

**Reglas recomendadas**:

- ✅ **Require a pull request before merging**
  - ✅ **Require approvals**: 1
  - ⚠️ **NO marcar** "Dismiss stale approvals" (más flexible para desarrollo)

- ✅ **Require status checks to pass before merging**
  - **Status checks que deben pasar**:
    - `Test` (del workflow ci-develop.yml)

- ✅ **Require conversation resolution before merging**

- ❌ **NO require linear history** (permite merge commits para features)

- ✅ **Do not allow bypassing the above settings**

### Configuración para Feature Branches

**Branch name pattern**: `feature/*`, `fix/*`, `refactor/*`

**Reglas opcionales**:

- ✅ **Require a pull request before merging**
- ⚠️ Sin otras restricciones (máxima flexibilidad)

---

## Configuración de Secretos

### Secretos Requeridos

#### 1. PyPI API Token (Producción)

1. **Crear token en PyPI**:
   - Ve a [https://pypi.org/manage/account/token/](https://pypi.org/manage/account/token/)
   - Click en **Add API token**
   - **Token name**: `GitHub Actions - Colmena Production`
   - **Scope**: `Project: colmena`
   - Copia el token (empieza con `pypi-...`)

2. **Agregar a GitHub**:
   - Ve a tu repositorio en GitHub
   - **Settings** → **Secrets and variables** → **Actions**
   - Click en **New repository secret**
   - **Name**: `PYPI_API_TOKEN`
   - **Secret**: Pega el token de PyPI
   - Click en **Add secret**

#### 2. TestPyPI API Token (Staging)

1. **Crear cuenta en TestPyPI** (separada de PyPI):
   - Ve a [https://test.pypi.org/account/register/](https://test.pypi.org/account/register/)
   - Registra tu cuenta y verifica email

2. **Crear token en TestPyPI**:
   - Ve a [https://test.pypi.org/manage/account/token/](https://test.pypi.org/manage/account/token/)
   - Click en **Add API token**
   - **Token name**: `GitHub Actions - Colmena Staging`
   - **Scope**: `Entire account` (primera vez) o `Project: colmena`
   - Copia el token (empieza con `pypi-...`)

3. **Agregar a GitHub**:
   - En tu repositorio: **Settings** → **Secrets and variables** → **Actions**
   - Click en **New repository secret**
   - **Name**: `TEST_PYPI_API_TOKEN`
   - **Secret**: Pega el token de TestPyPI
   - Click en **Add secret**

### Verificación

El secret `GITHUB_TOKEN` se proporciona automáticamente por GitHub Actions y no requiere configuración.

**Secretos configurados correctamente**:
- ✅ `PYPI_API_TOKEN` - Para publicación a PyPI (main)
- ✅ `TEST_PYPI_API_TOKEN` - Para publicación a TestPyPI (staging)
- ✅ `GITHUB_TOKEN` - Automático (para tags y releases)

---

## Flujo de Trabajo Recomendado

### 1. Desarrollo de Feature

```bash
# Crear rama desde develop
git checkout develop
git pull origin develop
git checkout -b feature/nueva-funcionalidad

# Desarrollar y hacer commits
git add .
git commit -m "feat: add new functionality"

# Push y crear PR a develop
git push -u origin feature/nueva-funcionalidad
```

En GitHub:
- Crear **Pull Request** a `develop`
- Esperar a que pase CI
- Solicitar review
- Merge cuando esté aprobado

### 2. Testing en Staging

```bash
# Asegurarse de que develop esté actualizado
git checkout develop
git pull origin develop

# Crear PR de develop → staging
```

En GitHub:
- Crear **Pull Request** de `develop` → `staging`
- **Título del PR**: Descriptivo del conjunto de features
  - Ejemplo: `feat: add Gemini support and improve streaming`
- Esperar a que pase CI
- Solicitar review
- Merge cuando esté aprobado

**Post-merge a staging**:
- Se genera versión pre-release automáticamente (ej: `0.1.0.dev20251004120000`)
- Se publica a TestPyPI
- Equipo de QA puede instalar y testear:
  ```bash
  # Instalar desde TestPyPI
  pip install -i https://test.pypi.org/simple/ colmena-ai==0.1.0.dev20251004120000
  ```

### 3. Preparar Release a Producción

Una vez validado en staging:

```bash
# Asegurarse de que staging esté actualizado
git checkout staging
git pull origin staging

# Crear PR de staging → main
```

En GitHub:
- Crear **Pull Request** de `staging` → `main`
- **Título del PR debe seguir Conventional Commits**:
  - `feat: add support for new LLM providers` (MINOR bump)
  - `fix: resolve critical bug in streaming` (PATCH bump)
  - `feat!: redesign API interface` (MAJOR bump)
- Esperar CI
- Solicitar review final
- **Importante**: El **último commit** que llegue a `main` determinará el bump de versión

### 4. Merge a Main (Release Producción)

Cuando se hace merge a `main`:

```bash
# Opción 1: Usar "Squash and merge" (RECOMENDADO)
# - Combina todos los commits en uno
# - El mensaje del squash determina el bump
# - Formato: "feat: add multiple features (#123)"

# Opción 2: Usar "Merge commit"
# - El último commit del PR determina el bump
# - Asegúrate de que el último commit tenga el tipo correcto
```

### 5. Proceso Automático Post-Merge a Main

1. ✅ GitHub Actions ejecuta CI completo
2. 📊 Calcula nueva versión según Conventional Commit
3. 📝 Actualiza `pyproject.toml` y `Cargo.toml`
4. 💾 Crea commit: `chore: bump version to X.Y.Z`
5. 🏷️ Crea tag: `vX.Y.Z`
6. 📦 Construye wheels de Python
7. 🚀 Publica a PyPI (producción)
8. 🎉 Crea GitHub Release con binarios

### 6. Verificación de Release

```bash
# Verificar en PyPI
pip install colmena --upgrade
python -c "import colmena; print(colmena.__version__)"

# Verificar en GitHub
# - Ir a Releases
# - Debe aparecer vX.Y.Z con fecha reciente
```

---

## Troubleshooting

### ❌ El workflow falla en "Semantic Version Bump"

**Problema**: No se puede parsear la versión de `pyproject.toml`

**Solución**:
```bash
# Verificar formato en pyproject.toml
grep version pyproject.toml
# Debe ser: version = "0.1.0"
```

### ❌ El workflow falla en "Publish to PyPI"

**Problema**: Token inválido o expirado

**Solución**:
1. Regenerar token en PyPI
2. Actualizar secret `PYPI_API_TOKEN` en GitHub

### ❌ El workflow falla en "Commit version bump"

**Problema**: Permisos insuficientes para GitHub Actions

**Solución**:
- Settings → Actions → General
- Workflow permissions → **Read and write permissions**

### ❌ El versionado no es correcto

**Problema**: El commit message no sigue Conventional Commits

**Solución**:
```bash
# Revisar el último commit en main
git log -1 --pretty=%B

# Debe empezar con: feat:, fix:, feat!:, etc.
```

### ❌ CI pasa en develop pero falla en main

**Problema**: Diferencias entre ramas o configuración

**Solución**:
```bash
# Sincronizar develop con main antes de merge
git checkout develop
git merge main
git push origin develop
```

---

## Mejores Prácticas

### Commits

1. **Usa Conventional Commits siempre**
2. **Sé específico en el scope**: `feat(providers):`, `fix(streaming):`
3. **Describe el "por qué" en el body** (si es necesario)
4. **Una feature = un commit** (en PR con squash)

### Pull Requests

1. **Títulos descriptivos siguiendo Conventional Commits**
2. **Descripción clara del cambio**
3. **Tests incluidos**
4. **Un PR = una feature/fix** (no mezclar)

### Releases

1. **Merge a main solo cuando esté 100% listo**
2. **Revisar que el tipo de commit sea correcto antes de merge**
3. **Usar "Squash and merge" para control del mensaje**
4. **No hacer push directo a main** (usar PRs)

### Versionado

1. **Breaking changes son raros**: Piensa dos veces antes de `feat!:`
2. **Features grandes pueden ser v0.X.0**: No necesitan ser v1.0.0
3. **Pre-releases usan `-alpha`, `-beta`**: Ejemplo: `v1.0.0-beta.1`

---

## Recursos Adicionales

- [Semantic Versioning 2.0.0](https://semver.org/)
- [Conventional Commits Specification](https://www.conventionalcommits.org/)
- [GitHub Actions Documentation](https://docs.github.com/en/actions)
- [Maturin Documentation](https://www.maturin.rs/)
- [PyPI Publishing Guide](https://packaging.python.org/tutorials/packaging-projects/)

---

## Resumen Visual

```
┌───────────────────────────────────────────────────────────────────────────────┐
│  Flujo Completo de CI/CD con Staging                                          │
└───────────────────────────────────────────────────────────────────────────────┘

Feature Branch          Develop              Staging                Main
──────────────         ────────             ────────              ──────
     │                     │                    │                    │
     │  feat: feature A    │                    │                    │
     ├────────────────────►│                    │                    │
     │                     │  CI ✓              │                    │
     │                     │  Tests ✓           │                    │
     │                     │                    │                    │
     │  feat: feature B    │                    │                    │
     ├────────────────────►│                    │                    │
     │                     │  CI ✓              │                    │
     │                     │                    │                    │
     │                     │  Ready for staging │                    │
     │                     ├───────────────────►│                    │
     │                     │                    │  CI ✓              │
     │                     │                    │  Tests ✓           │
     │                     │                    │  Version: 1.0.0-rc.xxx
     │                     │                    │  TestPyPI Publish ✓│
     │                     │                    │  QA Testing...     │
     │                     │                    │                    │
     │                     │                    │  ✅ Validated      │
     │                     │                    ├───────────────────►│
     │                     │                    │                    │  CI ✓
     │                     │                    │                    │  Tests ✓
     │                     │                    │                    │  Version: 1.0.0 → 1.1.0
     │                     │                    │                    │  Commit: "chore: bump v1.1.0"
     │                     │                    │                    │  Tag: v1.1.0
     │                     │                    │                    │  PyPI Publish ✓
     │                     │                    │                    │  GitHub Release ✓
```

---

## Contacto y Soporte

Para preguntas o problemas con el flujo de CI/CD:
- Abrir un issue en GitHub
- Revisar esta documentación
- Consultar con el equipo de DevOps

---

**Última actualización**: 2025-10-04
**Versión del documento**: 1.0.0
