# Git Hooks y Conventional Commits

Esta guía explica cómo configurar y usar los git hooks para validar commits según el estándar de Conventional Commits.

## ¿Qué son los Git Hooks?

Los git hooks son scripts que se ejecutan automáticamente en ciertos eventos de git (como `commit`, `push`, etc.). En este proyecto usamos un hook `commit-msg` para validar que todos los commits sigan el formato de [Conventional Commits](https://www.conventionalcommits.org/).

## ¿Por qué Conventional Commits?

- ✅ **Versionado automático**: El CI/CD usa los commits para determinar el tipo de bump (MAJOR, MINOR, PATCH)
- ✅ **Changelog automático**: Facilita generar logs de cambios
- ✅ **Historial legible**: Commits consistentes y fáciles de entender
- ✅ **Colaboración**: Todos siguen el mismo estándar

## Instalación

### Primera vez (obligatorio)

Cuando clones el repositorio o actualices, ejecuta:

```bash
./scripts/install-hooks.sh
```

Verás:

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
| `docs` | Cambios en documentación | - |
| `style` | Formato, linting (sin cambio de código) | - |
| `refactor` | Refactorización sin cambio funcional | PATCH |
| `perf` | Mejora de rendimiento | PATCH |
| `test` | Agregar o modificar tests | - |
| `build` | Cambios en sistema de build o dependencias | - |
| `ci` | Cambios en CI/CD | - |
| `chore` | Tareas de mantenimiento | - |
| `revert` | Revertir un commit anterior | - |

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
| `docs: update` | Sin bump |

Ver [10_cicd_guide.md](./10_cicd_guide.md) para más detalles.

## Troubleshooting

### El hook no se ejecuta

```bash
# Reinstalar hooks
./scripts/install-hooks.sh

# Verificar permisos
ls -la .git/hooks/commit-msg
# Debe mostrar: -rwxr-xr-x

# Dar permisos manualmente
chmod +x .git/hooks/commit-msg
```

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
- [Guía de CI/CD del proyecto](../CICD_GUIDE.md)

---

**Última actualización**: 2025-10-04
