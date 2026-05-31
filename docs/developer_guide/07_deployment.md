# 📦 Deployment y Distribución

Esta guía detalla cómo compilar, empaquetar y desplegar **Colmena** en producción, cubriendo desde la generación de *wheels* de Python hasta la contenedorización con Docker.

## 🐍 Building Wheels (Maturin)

Colmena es un proyecto híbrido que compila Rust en binarios nativos accesibles a través de Python. Usamos `maturin` para el empaquetado.

```bash
# Build para la plataforma actual (para desarrollo local)
maturin build --release

# Build para múltiples plataformas (usualmente en CI/CD)
maturin build --release --target x86_64-unknown-linux-gnu
maturin build --release --target x86_64-pc-windows-msvc
maturin build --release --target x86_64-apple-darwin
maturin build --release --target aarch64-apple-darwin

# Los archivos .whl resultantes se encontrarán en:
ls target/wheels/
```

## 🐋 Contenedorización (Docker)

Para despliegues en la nube o servidores on-premise, puedes usar la imagen oficial de **Colmena DAG Engine**.

### Dockerfile (Multi-stage)

Usamos un `Dockerfile` optimizado que utiliza una imagen de construcción pesada y una imagen de ejecución ligera (`debian:bookworm-slim`).

```dockerfile
# (Ver Dockerfile en la raíz del proyecto para la versión completa)
FROM rust:1.75-slim-bookworm as builder
...
RUN cargo build --release --bin dag_engine --manifest-path src/libs/colmena/Cargo.toml

FROM debian:bookworm-slim as runtime
...
ENTRYPOINT ["/app/dag_engine"]
CMD ["serve", "/app/graph.json", "--host", "0.0.0.0", "--port", "3000"]
```

### Docker Compose

El stack completo incluye PostgreSQL para la gestión de estados y persistencia.

```yaml
# Levantar el motor y la base de datos
docker-compose up -d
```

## 🚀 GitHub Actions (CI/CD)

El proyecto utiliza workflows automatizados en `.github/workflows/`:

1.  **`ci-develop.yml`**: Ejecuta tests y linting en cada pull request a `develop`.
2.  **`ci-staging.yml`**: Pruebas de integración exhaustivas antes de pasar a `main`.
3.  **`cd-main.yml`**: Orquestador principal de despliegue que realiza:
    *   **Versionado Automático**: Basado en [Conventional Commits](./09_git_hooks.md).
    *   **Publicación en PyPI**: Genera wheels para Linux, macOS y Windows.
    *   **Publicación en NPM**: Genera bindings para el ecosistema Node.js.
    *   **GitHub Release**: Crea una release con el changelog generado automáticamente.

## 🔐 Configuración de Producción

En producción, es obligatorio configurar las siguientes variables de entorno:

| Variable | Descripción | Requerido |
| :--- | :--- | :--- |
| **`DATABASE_URL`** | URL de conexión a PostgreSQL (`postgres://user:pass@host:5432/db`) | Sí (Engine) |
| **`SECURE_VALUES_KEY`** | Clave de 32 bytes (base64) para cifrar/descifrar secretos (AES-GCM) | Sí |
| **`OPENAI_API_KEY`** | API Key del proveedor OpenAI | Opcional |
| **`GEMINI_API_KEY`** | API Key de Google Gemini | Opcional |
| **`ANTHROPIC_API_KEY`** | API Key de Anthropic Claude | Opcional |
| **`RUST_LOG`** | Nivel de logs (`debug`, `info`, `warn`, `error`) | `info` |

### Generar `SECURE_VALUES_KEY`

```bash
# Ejemplo: Generar una clave segura aleatoria
openssl rand -base64 32
```

## 🏷️ Estrategia de Versionado

Colmena sigue **SemVer (Semantic Versioning 2.0.0)**. El CI/CD automatiza los cambios de versión:

```toml
# Cargo.toml / pyproject.toml / package.json
version = "0.3.2"    # MAJOR.MINOR.PATCH
```

*   **PATCH**: `fix: ...` o `perf: ...` (No rompe API, corrige errores).
*   **MINOR**: `feat: ...` (Añade funcionalidad compatible).
*   **MAJOR**: `feat!: ...` o `fix!: ...` (Rompe compatibilidad hacia atrás).

---

**🐝 Colmena** - *Infraestructura robusta para aplicaciones de IA deterministas.*


