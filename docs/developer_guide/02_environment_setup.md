# ⚙️ Configuración del Entorno de Desarrollo

### Setup Inicial
## 🖥️ Requisitos del Sistema

- **RAM**: 2GB mínimo, 4GB recomendado
- **CPU**: x86_64 o ARM64
- **Rust**: 1.70+ (recomendado 1.75+)
- **Python**: 3.8+ (recomendado 3.11+)

## 🔧 Instalación por Sistema Operativo

### 🐧 Linux (Ubuntu/Debian)

```bash
sudo apt update && sudo apt install -y curl build-essential python3 python3-dev python3-pip python3-venv pkg-config libssl-dev git
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

### 🍎 macOS

```bash
xcode-select --install
/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
brew install python@3.11
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

### 🪟 Windows (PowerShell)

1. Instala **Visual Studio Build Tools 2022** con la carga "C++ build tools".
2. Ejecuta en PowerShell con permisos de administrador:
```powershell
choco install python rust-msvc git -y
```

## 🏗️ Compilación Paso a Paso

### 1. Entorno Python

```bash
python3 -m venv venv
source venv/bin/activate  # Linux/macOS
# venv\Scripts\activate     # Windows
pip install --upgrade pip maturin
```

### 2. Compilar Bindings (Maturin)

Colmena es un proyecto híbrido. **No uses `cargo build` directamente** para enlazar con Python.

```bash
# Modo Desarrollo: Instala directamente en el venv
maturin develop

# Modo Release: Genera el archivo .whl optimizado
maturin build --release
```

### 🔐 Variables de Entorno

El motor requiere configurar las siguientes variables de entorno (puedes usar un archivo `.env` en la raíz):

| Variable | Requerida | Propósito |
| :--- | :--- | :--- |
| `SECURE_VALUES_KEY` | **SÍ** | Clave de 32 bytes en Base64 para cifrado AES-GCM. |
| `DATABASE_URL` | Opcional | Conexión a PostgreSQL para persistencia de memoria. |
| `OPENAI_API_KEY` | Opcional | Requerida si usas el proveedor OpenAI. |
| `GEMINI_API_KEY` | Opcional | Requerida si usas el proveedor Google Gemini. |
| `ANTHROPIC_API_KEY` | Opcional | Requerida si usas el proveedor Anthropic Claude. |

### Cómo generar la `SECURE_VALUES_KEY`
Puedes usar este comando para generar una clave válida:
```bash
openssl rand -base64 32
```

### 🗄️ Inicialización de la Base de Datos

El framework utiliza PostgreSQL (o SQLite localmente) para persistir la memoria de los LLMs, el historial de corridas y el estado de ejecución de los agentes. Si utilizas PostgreSQL, sigue estos pasos para inicializar tu base de datos desde cero:

1. Asegúrate de tener una instancia de PostgreSQL corriendo.
2. Configura la variable `DATABASE_URL` en tu `.env`. Ejemplo:
   ```bash
   DATABASE_URL="postgres://usuario:password@localhost/colmena"
   ```
3. Instala la herramienta de migraciones de `sqlx` (si aún no la tienes):
   ```bash
   cargo install sqlx-cli --no-default-features --features rustls,postgres
   ```
4. Crea la base de datos y ejecuta la migración inicial:
   ```bash
   sqlx database create
   sqlx migrate run --source src/libs/colmena/migrations/postgres
   ```

> **Nota:** Todos los esquemas necesarios (`llm_node_history`, `dag_task_memory`, etc.) se encuentran agrupados en un único archivo inicial, por lo que este comando dejará la base de datos completamente configurada para el motor.

### Scripts de Desarrollo

Para facilitar el ciclo de desarrollo, se recomiendan los siguientes comandos:

```bash
# Auto-chequeo y tests de Rust al guardar
cargo watch -x "check" -x "test"

# Recompilación rápida de bindings para pruebas en Python
maturin develop

# Ejecución de un grafo de prueba con el CLI del DAG Engine
cargo run --bin dag_engine -- run tests/graphs/basic/trigger.json
```

### Configuración del Editor

**VS Code (settings.json):**
```json
{
    "rust-analyzer.cargo.features": "all",
    "rust-analyzer.checkOnSave.command": "clippy",
    "rust-analyzer.linkedProjects": [
        "src/libs/colmena/Cargo.toml"
    ],
    "python.defaultInterpreterPath": "./.venv/bin/python"
}
```

**Vim/Neovim:**
```lua
-- rust-tools.nvim setup
require('rust-tools').setup({
    server = {
        settings = {
            ["rust-analyzer"] = {
                cargo = { features = "all" },
                checkOnSave = { command = "clippy" }
            }
        }
    }
})
```
