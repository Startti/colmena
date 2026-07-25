# Git Hooks y Conventional Commits

Esta guía explica cómo configurar y usar los git hooks para validar commits según el estándar de Conventional Commits.

## ¿Qué son los Git Hooks?

Los git hooks son scripts que se ejecutan automáticamente en ciertos eventos de git (como `commit`, `push`, etc.). En este proyecto usamos hooks de husky: un hook `commit-msg` para validar que todos los commits sigan el formato de [Conventional Commits](https://www.conventionalcommits.org/), y un hook `pre-commit` que regenera automáticamente `docs/agent_context/module_dependency_map.md` (vía `scripts/gen_module_map.py`) cuando los cambios en stage tocan archivos `.rs` o el generador.

## ¿Por qué Conventional Commits?

- ✅ **Versionado automático**: El CI/CD usa los commits para determinar el tipo de bump (MAJOR, MINOR, PATCH)
- ✅ **Changelog automático**: Facilita generar logs de cambios
- ✅ **Historial legible**: Commits consistentes y fáciles de entender
- ✅ **Colaboración**: Todos siguen el mismo estándar

## Instalación

### Primera vez (obligatorio)

Los hooks se ejecutan mediante husky con `core.hooksPath=.husky`. Verifica que esté configurado con `git config core.hooksPath` (debe devolver `.husky`); si no, actívalo con `git config core.hooksPath .husky` (este comando no imprime salida). Los hooks activos son `.husky/commit-msg` y `.husky/pre-commit`.

El script fallback legacy `./scripts/install-hooks.sh` (copia solo `commit-msg` a `.git/hooks/`, ignorado cuando `core.hooksPath=.husky`) imprime:

```
Installing git hooks...
✅ Git hooks installed successfully!

Your commits will now be validated against Conventional Commits format:
  <type>[optional scope][!]: <description>

Examples:
  feat: add new feature
  fix(api): resolve bug
  feat!: breaking change
```

## Formato de Commits

### Estructura básica

```
<type>[optional scope][!]: <description>

[optional body]

[optional footer(s)]
```

### Tipos de commit

| Tipo | Descripción | Versión |
|------|-------------|---------|
| `feat` | Nueva funcionalidad | MINOR (1.0.0 → 1.1.0) |
| `fix` | Corrección de bug | PATCH (1.0.0 → 1.0.1) |
| `docs` | Cambios en documentación | PATCH |
| `style` | Formato, linting (sin cambio de código) | PATCH |
| `refactor` | Refactorización sin cambio funcional | PATCH |
| `perf` | Mejora de rendimiento | PATCH |
| `test` | Agregar o modificar tests | PATCH |
| `build` | Cambios en sistema de build o dependencias | PATCH |
| `ci` | Cambios en CI/CD | PATCH |
| `chore` | Tareas de mantenimiento | PATCH |
| `revert` | Revertir un commit anterior | PATCH |

> **Nota:** En este repo el workflow `cd-main.yml` sube **MAJOR** ante `feat!`/`BREAKING CHANGE`, **MINOR** ante `feat`, y **PATCH** para cualquier otro commit (incluye `fix`, `perf`, `refactor` y — por el `else` por defecto — `docs`, `style`, `test`, `build`, `ci`, `chore`, `revert`). Es decir, todo commit fusionado a `main` produce al menos un bump PATCH.

### Breaking Changes

Para indicar un cambio que rompe compatibilidad:

```bash
# Opción 1: Usar ! después del tipo
feat!: cambiar API de configuración

# Opción 2: Usar BREAKING CHANGE en el footer
feat: cambiar API de configuración

BREAKING CHANGE: El método config() ahora requiere un objeto en vez de string
```

Esto genera un **MAJOR** version bump (1.0.0 → 2.0.0).

## Ejemplos

### ✅ Commits válidos

```bash
# Feature simple
git commit -m "feat: add support for Gemini Flash model"

# Fix con scope
git commit -m "fix(streaming): resolve timeout in Anthropic adapter"

# Breaking change
git commit -m "feat!: redesign provider configuration API"

# Con body y footer
git commit -m "feat: add retry mechanism

Implements exponential backoff for API calls
Configurable max retries and delay

Closes #123"

# Refactoring
git commit -m "refactor(domain): simplify error handling"

# Documentation
git commit -m "docs: update API examples in README"

# Tests
git commit -m "test: add integration tests for OpenAI streaming"
```

### ❌ Commits inválidos (serán rechazados)

```bash
# Sin tipo
git commit -m "added new feature"
❌ Error: Commit message does not follow Conventional Commits format

# Tipo incorrecto
git commit -m "feature: add new feature"  # debe ser "feat"
❌ Error: Commit message does not follow Conventional Commits format

# Sin descripción
git commit -m "feat:"
❌ Error: Commit message does not follow Conventional Commits format

# Sin dos puntos
git commit -m "feat add new feature"
❌ Error: Commit message does not follow Conventional Commits format
```

## Scopes (Opcional)

Los scopes ayudan a categorizar los cambios:

```bash
# Por módulo
git commit -m "feat(llm): add temperature control"
git commit -m "fix(streaming): handle connection errors"

# Por capa
git commit -m "refactor(domain): simplify value objects"
git commit -m "test(infrastructure): add adapter tests"

# Por proveedor
git commit -m "feat(openai): add GPT-4o support"
git commit -m "fix(gemini): resolve streaming chunks"

# Por tipo de cambio
git commit -m "perf(cache): implement request memoization"
git commit -m "build(deps): update pyo3 to 0.21"
```

## Bypass del Hook (NO recomendado)

En casos excepcionales (como merges automáticos), puedes saltar la validación:

```bash
git commit --no-verify -m "mensaje sin formato"
```

⚠️ **Advertencia**: Esto afectará el versionado automático del CI/CD.

## Relación con CI/CD

El CI/CD lee el **último commit** del merge a `main` para determinar el bump:

| Commit | Version Bump |
|--------|--------------|
| `feat!: breaking change` | 1.0.0 → **2.0.0** (MAJOR) |
| `feat: new feature` | 1.0.0 → **1.1.0** (MINOR) |
| `fix: bug fix` | 1.0.0 → **1.0.1** (PATCH) |
| `docs: update` (y cualquier otro tipo) | 1.0.0 → **1.0.1** (PATCH por defecto) |

Ver [10_cicd_guide.md](./10_cicd_guide.md) para más detalles.

## Troubleshooting

### El hook no se ejecuta

```bash
# Verificar que git apunte a los hooks de husky
git config core.hooksPath
# Debe devolver: .husky

# Si no, activarlo
git config core.hooksPath .husky

# Verificar permisos
ls -la .husky/commit-msg .husky/pre-commit
# Deben mostrar: -rwxr-xr-x

# Dar permisos manualmente
chmod +x .husky/commit-msg .husky/pre-commit
```

> Nota: `./scripts/install-hooks.sh` es un fallback legacy que copia solo `commit-msg` a `.git/hooks/`; con `core.hooksPath=.husky` esa ruta se ignora, así que prefiere el mecanismo de husky de arriba.

### El hook rechaza commits válidos

Verifica el formato exacto:

```bash
# Debe tener:
# 1. Tipo válido (feat, fix, etc.)
# 2. Dos puntos (:)
# 3. Espacio después de los dos puntos
# 4. Descripción no vacía

# ✅ Correcto
feat: add feature

# ❌ Incorrecto (sin espacio)
feat:add feature

# ❌ Incorrecto (tipo inválido)
feature: add feature
```

### Commits de merge

Los commits de merge automáticos de GitHub no son validados. Solo se validan commits normales.

## Herramientas Complementarias

### Commitizen (Opcional)

Para generar commits interactivamente:

```bash
# Instalar globalmente
npm install -g commitizen cz-conventional-changelog

# Configurar en el proyecto
echo '{ "path": "cz-conventional-changelog" }' > .czrc

# Usar
git cz
# Te guiará paso a paso para crear el commit
```

### VSCode Extension

Instala la extensión [Conventional Commits](https://marketplace.visualstudio.com/items?itemName=vivaxy.vscode-conventional-commits) para ayuda en el editor.

## Recursos

- [Conventional Commits Specification](https://www.conventionalcommits.org/)
- [Semantic Versioning](https://semver.org/)
- [Angular Commit Guidelines](https://github.com/angular/angular/blob/main/CONTRIBUTING.md#commit)
- [Guía de CI/CD del proyecto](./10_cicd_guide.md)

---

**Última actualización**: 2025-10-04
