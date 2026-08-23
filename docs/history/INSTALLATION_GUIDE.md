# 📦 Guía de Instalación y Compilación - Colmena

Esta guía te ayudará a compilar e instalar Colmena en cualquier sistema operativo.

## 📋 Tabla de Contenidos

- [Requisitos del Sistema](#requisitos-del-sistema)
- [Instalación por Sistema Operativo](#instalación-por-sistema-operativo)
- [Compilación Paso a Paso](#compilación-paso-a-paso)
- [Verificación](#verificación)
- [Solución de Problemas](#solución-de-problemas)
- [Distribución](#distribución)

## 🖥️ Requisitos del Sistema

### Requisitos Mínimos

- **RAM**: 2GB mínimo, 4GB recomendado
- **Espacio en Disco**: 500MB para dependencias
- **CPU**: x86_64 o ARM64
- **Red**: Conexión a internet para descargar dependencias

### Software Necesario

- **Rust**: 1.70+ (recomendado 1.75+)
- **Python**: 3.8+ (recomendado 3.11+)
- **Git**: Para clonar el repositorio
- **Build Tools**: Específicos por sistema operativo

## 🔧 Instalación por Sistema Operativo

### 🐧 Linux (Ubuntu/Debian)

```bash
# 1. Actualizar sistema
sudo apt update && sudo apt upgrade -y

# 2. Instalar dependencias de compilación
sudo apt install -y \
    curl \
    build-essential \
    python3 \
    python3-dev \
    python3-pip \
    python3-venv \
    pkg-config \
    libssl-dev \
    git

# 3. Instalar Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.bashrc

# 4. Verificar instalación
rustc --version
python3 --version
```

### 🐧 Linux (CentOS/RHEL/Fedora)

```bash
# Para CentOS/RHEL 8+
sudo dnf groupinstall "Development Tools" -y
sudo dnf install -y python3 python3-devel python3-pip openssl-devel pkg-config git

# Para Fedora
sudo dnf install -y gcc gcc-c++ python3-devel python3-pip openssl-devel pkg-config git

# Instalar Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.bashrc
```

### 🍎 macOS

```bash
# 1. Instalar Xcode Command Line Tools
xcode-select --install

# 2. Instalar Homebrew (si no está instalado)
/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"

# 3. Instalar Python (opcional, macOS incluye Python 3)
brew install python@3.11

# 4. Instalar Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.bashrc

# 5. Verificar instalación
rustc --version
python3 --version
```

### 🪟 Windows

#### Opción 1: PowerShell (Recomendado)

```powershell
# 1. Instalar chocolatey (administrador)
Set-ExecutionPolicy Bypass -Scope Process -Force
[System.Net.ServicePointManager]::SecurityProtocol = [System.Net.ServicePointManager]::SecurityProtocol -bor 3072
iex ((New-Object System.Net.WebClient).DownloadString('https://community.chocolatey.org/install.ps1'))

# 2. Instalar dependencias
choco install python rust-msvc git -y

# 3. Instalar Visual Studio Build Tools
choco install visualstudio2022buildtools --package-parameters "--add Microsoft.VisualStudio.Workload.VCTools" -y
```

#### Opción 2: Manual

1. **Instalar Visual Studio Build Tools**:
   - Descargar desde: https://visualstudio.microsoft.com/visual-cpp-build-tools/
   - Seleccionar "C++ build tools" durante la instalación

2. **Instalar Rust**:
   - Ir a https://rustup.rs/
   - Descargar `rustup-init.exe`
   - Ejecutar y seguir las instrucciones

3. **Instalar Python**:
   - Descargar desde https://python.org
   - Versión 3.8+ (marcar "Add to PATH")

4. **Instalar Git**:
   - Descargar desde https://git-scm.com

## 🏗️ Compilación Paso a Paso

### 1. Obtener el Código Fuente

```bash
# Clonar repositorio
git clone https://github.com/tu-org/colmena.git
cd colmena

# Verificar contenido
ls -la
```

### 2. Configurar Entorno Python

```bash
# Crear entorno virtual
python3 -m venv venv

# Activar entorno virtual
# Linux/macOS:
source venv/bin/activate
# Windows:
venv\Scripts\activate

# Actualizar pip
pip install --upgrade pip

# Instalar maturin
pip install maturin
```

### 3. Compilar el Proyecto (Rust + Python)

Este es un proyecto híbrido de Rust y Python. **No uses `cargo build` directamente**, ya que no enlazará correctamente las librerías de Python y fallará.

La herramienta correcta para compilar es `maturin`, que se encarga de orquestar la compilación de Rust y la creación de los bindings de Python.

#### Opción 1: Desarrollo (Recomendado)

Este comando compila el código Rust y lo instala en tu entorno virtual actual. Es la forma más rápida de tener los últimos cambios disponibles en Python.

```bash
# Compila e instala en el venv actual
maturin develop
```

#### Opción 2: Producción

Si quieres generar un archivo `.whl` para distribución, usa `maturin build`.

```bash
# Compilar en modo release (optimizado)
maturin build --release

# El "wheel" se encontrará en `target/wheels/`
ls target/wheels/
```

Para verificar que el código Rust compila de forma independiente (sin los bindings de Python), puedes usar `cargo check`.

```bash
# Verificar que el código Rust es válido
cargo check

# Ejecutar los tests de Rust
cargo test
```

### 5. Verificar Instalación

```bash
# Test básico de importación
python -c "import colmena; print('✅ Colmena instalado')"

# Test de funcionalidad
python -c "
import colmena
llm = colmena.ColmenaLlm()
print('✅ ColmenaLlm inicializado:', type(llm))
print('✅ Métodos disponibles:', [m for m in dir(llm) if not m.startswith('_')])
"
```

## ✅ Verificación Completa

### Script de Verificación

Crea y ejecuta este script para verificar todo:

```python
# verify_installation.py
import sys
import subprocess
import importlib

def run_command(cmd):
    """Ejecutar comando y capturar salida"""
    try:
        result = subprocess.run(cmd, shell=True, capture_output=True, text=True)
        return result.returncode == 0, result.stdout, result.stderr
    except Exception as e:
        return False, "", str(e)

def check_requirement(name, cmd, version_cmd=None):
    """Verificar que un requisito esté instalado"""
    print(f"🔍 Verificando {name}...")

    success, stdout, stderr = run_command(cmd)
    if success:
        if version_cmd:
            _, version_out, _ = run_command(version_cmd)
            print(f"  ✅ {name} instalado: {version_out.strip()}")
        else:
            print(f"  ✅ {name} instalado")
        return True
    else:
        print(f"  ❌ {name} NO encontrado")
        if stderr:
            print(f"     Error: {stderr.strip()}")
        return False

def check_python_module(module_name):
    """Verificar que un módulo Python esté disponible"""
    try:
        importlib.import_module(module_name)
        print(f"  ✅ Módulo {module_name} disponible")
        return True
    except ImportError:
        print(f"  ❌ Módulo {module_name} NO disponible")
        return False

def main():
    print("🐝 Colmena - Verificación de Instalación")
    print("=" * 50)

    # Verificar requisitos del sistema
    print("\n📋 Verificando Requisitos del Sistema:")

    checks = [
        ("Python", "python --version", "python --version"),
        ("Rust", "rustc --version", "rustc --version"),
        ("Cargo", "cargo --version", "cargo --version"),
        ("Git", "git --version", "git --version"),
        ("Pip", "pip --version", "pip --version"),
    ]

    all_good = True
    for name, cmd, version_cmd in checks:
        if not check_requirement(name, cmd, version_cmd):
            all_good = False

    # Verificar módulos Python
    print("\n📦 Verificando Módulos Python:")
    python_modules = ["maturin", "colmena"]

    for module in python_modules:
        if not check_python_module(module):
            all_good = False

    # Test funcional de Colmena
    print("\n🧪 Test Funcional de Colmena:")
    try:
        import colmena
        llm = colmena.ColmenaLlm()
        print(f"  ✅ ColmenaLlm inicializado: {type(llm)}")
        print(f"  ✅ Archivo: {colmena.__file__}")
        print(f"  ✅ Métodos: {[m for m in dir(llm) if not m.startswith('_')]}")
    except Exception as e:
        print(f"  ❌ Error inicializando Colmena: {e}")
        all_good = False

    # Verificar que es código nativo
    print("\n🔧 Verificando Compilación Nativa:")
    try:
        import colmena
        import inspect
        llm = colmena.ColmenaLlm()

        try:
            source = inspect.getsource(llm.call)
            print("  ❌ Método call() está en Python (no nativo)")
            all_good = False
        except (OSError, TypeError):
            print(f"  ✅ Método call() es nativo: {type(llm.call)}")
    except Exception as e:
        print(f"  ❌ Error verificando código nativo: {e}")
        all_good = False

    # Resultado final
    print("\n" + "=" * 50)
    if all_good:
        print("🎉 ¡INSTALACIÓN COMPLETAMENTE EXITOSA!")
        print("✅ Todos los componentes están funcionando correctamente")
        print("🚀 Colmena está listo para usar")
    else:
        print("⚠️  HAY PROBLEMAS CON LA INSTALACIÓN")
        print("❌ Revisa los errores anteriores")
        print("📖 Consulta la guía de solución de problemas")

    return all_good

if __name__ == "__main__":
    success = main()
    sys.exit(0 if success else 1)
```

```bash
# Ejecutar verificación completa
python verify_installation.py
```

## 🚨 Solución de Problemas

### Error: "Microsoft Visual C++ 14.0 is required" (Windows)

```bash
# Instalar Visual Studio Build Tools
# Opción 1: Con chocolatey
choco install visualstudio2022buildtools --package-parameters "--add Microsoft.VisualStudio.Workload.VCTools"

# Opción 2: Manual
# Descargar desde: https://visualstudio.microsoft.com/visual-cpp-build-tools/
# Instalar con "C++ build tools" seleccionado
```

### Error: "python3-dev not found" (Linux)

```bash
# Ubuntu/Debian
sudo apt install python3-dev python3-pip

# CentOS/RHEL/Fedora
sudo dnf install python3-devel python3-pip
```

### Error: "maturin not found"

```bash
# Verificar que estás en el entorno virtual
source venv/bin/activate  # Linux/macOS
# venv\Scripts\activate     # Windows

# Reinstalar maturin
pip uninstall maturin
pip install --upgrade pip
pip install maturin
```

### Error: "Failed to find a supported Python installation"

```bash
# Verificar versión de Python (debe ser 3.8+)
python --version

# Si es muy antigua, instalar versión más nueva
# En Ubuntu:
sudo apt install python3.11 python3.11-dev python3.11-venv

# Crear nuevo entorno virtual con la versión correcta
python3.11 -m venv venv
source venv/bin/activate
pip install maturin
```

### Error: "Cargo.toml not found"

```bash
# Verificar que estás en el directorio correcto
pwd
ls -la

# Debe mostrar Cargo.toml en el directorio actual
# Si no, navegar al directorio correcto:
cd path/to/colmena
```

### Error: "OpenSSL not found" (Linux)

```bash
# Ubuntu/Debian
sudo apt install libssl-dev pkg-config

# CentOS/RHEL/Fedora
sudo dnf install openssl-devel pkg-config
```

### Error: "Permission denied" (Linux/macOS)

```bash
# Agregar permisos de ejecución
chmod +x venv/bin/activate

# O reinstalar en directorio con permisos
sudo chown -R $USER:$USER /path/to/colmena
```

### Error de Compilación con PyO3

```bash
# Limpiar caché y recompilar
cargo clean
rm -rf target/
rm -rf venv/

# Crear nuevo entorno y recompilar
python3 -m venv venv
source venv/bin/activate
pip install --upgrade pip maturin
maturin develop --release
```

## 📦 Distribución

### Crear Wheel para Distribución

```bash
# Compilar wheel optimizado
maturin build --release

# El archivo .whl se creará en target/wheels/
ls target/wheels/

# Instalar desde wheel
pip install target/wheels/colmena-*.whl
```

### Distribución en PyPI (Futuro)

```bash
# Compilar para múltiples plataformas
maturin build --release --target x86_64-unknown-linux-gnu
maturin build --release --target x86_64-pc-windows-msvc
maturin build --release --target x86_64-apple-darwin

# Subir a PyPI (cuando esté listo)
maturin publish
```

## 🎯 Próximos Pasos

Una vez que tengas Colmena instalado exitosamente:

1. **Leer la documentación de uso**: [🐍 Ejemplos de Uso en Python](../examples/python_usage.md) o [📖 Ejemplos Generales](../examples/USAGE_EXAMPLES.md)
2. **Configurar API keys**: Variables de entorno o configuración directa
3. **Ejecutar tests**: `python python/tests/test_streaming_scenarios.py`
4. **Explorar ejemplos**: Revisar los scripts de ejemplo incluidos
5. **Desarrollar tu aplicación**: ¡Empieza a construir con Colmena!

---

**🐝 Colmena** - *Tu puerta de entrada al mundo de la orquestación de IA en Rust*