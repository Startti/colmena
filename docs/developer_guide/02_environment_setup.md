# ⚙️ Configuración del Entorno de Desarrollo

### Setup Inicial

```bash
# 1. Clonar repositorio
git clone https://github.com/tu-org/colmena.git
cd colmena

# 2. Instalar herramientas de desarrollo
cargo install cargo-watch    # Auto-recompilación
cargo install cargo-expand   # Expansión de macros
cargo install clippy         # Linter avanzado

# 3. Configurar pre-commit hooks
cargo install pre-commit
pre-commit install

# 4. Setup Python
python3 -m venv .venv
source .venv/bin/activate
pip install -e '.[dev]'
```

### Scripts de Desarrollo

```bash
# scripts/dev.sh
#!/bin/bash

# Auto-recompilación en desarrollo
cargo watch -x "check" -x "test" -x "run"

# Compilación y test rápido
cargo check && cargo test && maturin develop

# Test completo con coverage
cargo test -- --nocapture
```

### Configuración del Editor

**VS Code (settings.json):**
```json
{
    "rust-analyzer.cargo.features": "all",
    "rust-analyzer.checkOnSave.command": "clippy",
    "python.defaultInterpreterPath": "./.venv/bin/python",
    "python.linting.enabled": true,
    "python.linting.mypyEnabled": true
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
