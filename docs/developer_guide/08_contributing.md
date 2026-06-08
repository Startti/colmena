# 🤝 Contribuir al Proyecto

¡Gracias por tu interés en mejorar Colmena! Como un proyecto de alto rendimiento que combina Rust y Python, seguimos estándares estrictos para mantener la calidad y la seguridad del motor.

## 🛠️ Entorno de Desarrollo

Antes de empezar, asegúrate de tener instalado:
- **Rust** 1.75+ (y `cargo-fmt`, `clippy`)
- **Python** 3.8+
- **Maturin** para los bindings.
- **Node.js** 20+ (si trabajas en los bindings de Node).

Ejecuta el setup inicial:
```bash
./scripts/install-hooks.sh # Activa validación de commits
pip install -e ".[dev]"     # Instala dependencias de desarrollo
```

## 🔄 Proceso de Pull Request

1.  **Fork y Branch**: Crea una rama desde `develop`.
    *   `feature/nombre-funcionalidad`
    *   `fix/descripcion-bug`
2.  **Conventional Commits**: Es obligatorio seguir el estándar de [Git Hooks y Commits](./08a_git_hooks.md). El CI/CD depende de esto para el versionado.
3.  **Calidad de Código**:
    *   **Rust**: Ejecuta `cargo fmt --all` y `cargo clippy -- -D warnings`.
    *   **Python**: Usamos `black` para formato y `ruff` para linting.
4.  **Tests**: Ningún PR es aceptado sin tests que validen la nueva lógica.
    *   `cargo test` (Core)
    *   `pytest tests/` (Integración Python)
5.  **Documentación**: Si cambias interfaces, actualiza la Guía del Desarrollador.

## 🔍 Checklist de Review

- [ ] ¿El código sigue las [Convenciones de Código](./03_coding_conventions.md)?
- [ ] ¿Se han añadido tests unitarios e integración?
- [ ] ¿`cargo clippy` pasa sin warnings?
- [ ] ¿El impacto en performance ha sido evaluado?
- [ ] ¿El manejo de errores usa `anyhow` o `thiserror` correctamente?
- [ ] ¿Se ha mantenido la compatibilidad hacia atrás o se ha marcado como `breaking change`?

## 🌲 Estrategia de Ramas

- **`main`**: Versiones estables y publicadas.
- **`develop`**: Rama principal de desarrollo e integración.
- **`feature/*` / `fix/*`**: Ramas de trabajo volátiles.

---

**🐝 Colmena** - *Construyendo el futuro de la orquestación de IA*

> 💡 **Para Desarrolladores**: Siempre prioriza la claridad del código sobre la optimización prematura. Rust ya es rápido por defecto; hazlo robusto primero.
