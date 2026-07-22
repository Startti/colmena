# 🔧 Guía de Solución de Problemas - Colmena

Esta guía te ayudará a resolver los problemas más comunes al compilar, instalar y usar Colmena.

## 📋 Tabla de Contenidos

- [Problemas de Instalación](#problemas-de-instalación)
- [Problemas de Compilación](#problemas-de-compilación)
- [Problemas de Ejecución](#problemas-de-ejecución)
- [Problemas con API Keys](#problemas-con-api-keys)
- [Problemas de Performance](#problemas-de-performance)
- [Problemas con Migraciones de Base de Datos](#-problemas-con-migraciones-de-base-de-datos)
- [Problemas con Gemini Streaming y Tool Calling](#-problemas-con-gemini-streaming-y-tool-calling)
- [Problemas con Secure Values en HTTP Tools](#-problemas-con-secure-values-en-http-tools)
- [Diagnóstico Avanzado](#diagnóstico-avanzado)
- [Obtener Ayuda](#obtener-ayuda)

## 🚨 Diagnóstico Rápido

Antes de buscar soluciones específicas, ejecuta este script de diagnóstico:

```python
# quick_diagnosis.py
import sys
import subprocess
import platform

def run_cmd(cmd):
    try:
        result = subprocess.run(cmd, shell=True, capture_output=True, text=True)
        return result.returncode == 0, result.stdout.strip(), result.stderr.strip()
    except:
        return False, "", "Command failed"

def quick_diagnosis():
    print("🔍 DIAGNÓSTICO RÁPIDO DE COLMENA")
    print("=" * 50)

    # Información del sistema
    print(f"🖥️  Sistema: {platform.system()} {platform.release()}")
    print(f"🐍 Python: {sys.version}")

    # Verificar componentes clave
    checks = [
        ("Rust", "rustc --version"),
        ("Cargo", "cargo --version"),
        ("Maturin", "maturin --version"),
        ("Git", "git --version")
    ]

    for name, cmd in checks:
        success, output, error = run_cmd(cmd)
        status = "✅" if success else "❌"
        print(f"{status} {name}: {output if success else 'NO ENCONTRADO'}")

    # Verificar Colmena
    try:
        import colmena
        print(f"✅ Colmena: Importado correctamente desde {colmena.__file__}")

        llm = colmena.ColmenaLlm()
        print(f"✅ ColmenaLlm: Inicializado correctamente")
    except Exception as e:
        print(f"❌ Colmena: Error - {e}")

    print("\n" + "=" * 50)

quick_diagnosis()
```

## 🛠️ Problemas de Instalación

### Error: "Command 'rustc' not found"

**Síntomas:**
```bash
bash: rustc: command not found
```

**Solución:**

```bash
# 1. Instalar Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# 2. Reinicializar shell
source ~/.bashrc
# o
source ~/.zshrc

# 3. Verificar instalación
rustc --version
```

**Solución alternativa (Windows):**
1. Descargar Rust desde https://rustup.rs/
2. Ejecutar instalador como administrador
3. Reiniciar terminal

### Error: "Microsoft Visual C++ 14.0 is required" (Windows)

**Síntomas:**
```
error: Microsoft Visual C++ 14.0 is required. Get it with "Microsoft Visual Studio Build Tools"
```

**Solución Método 1 (Recomendado):**
```powershell
# Instalar con chocolatey
Set-ExecutionPolicy Bypass -Scope Process -Force
iex ((New-Object System.Net.WebClient).DownloadString('https://community.chocolatey.org/install.ps1'))
choco install visualstudio2022buildtools --package-parameters "--add Microsoft.VisualStudio.Workload.VCTools" -y
```

**Solución Método 2 (Manual):**
1. Ir a https://visualstudio.microsoft.com/visual-cpp-build-tools/
2. Descargar "Build Tools for Visual Studio 2022"
3. Ejecutar instalador
4. Seleccionar "C++ build tools" y "Windows 10 SDK"
5. Instalar y reiniciar

### Error: "python3-dev not found" (Linux)

**Síntomas:**
```bash
Package python3-dev is not available
```

**Solución Ubuntu/Debian:**
```bash
sudo apt update
sudo apt install python3-dev python3-pip build-essential
```

**Solución CentOS/RHEL/Fedora:**
```bash
# CentOS/RHEL
sudo dnf install python3-devel python3-pip gcc gcc-c++

# Fedora
sudo dnf install python3-devel python3-pip @development-tools
```

### Error: "maturin: command not found"

**Síntomas:**
```bash
bash: maturin: command not found
```

**Solución:**
```bash
# Verificar que el entorno virtual está activado
source venv/bin/activate

# Instalar maturin
pip install --upgrade pip
pip install maturin

# Verificar instalación
maturin --version
```

## ⚙️ Problemas de Compilación

### Error: "failed to compile `colmena`"

**Síntomas:**
```
error[E0433]: failed to resolve: use of undeclared crate or module `tokio`
```

**Solución:**
```bash
# 1. Limpiar caché de compilación
cargo clean
rm -rf target/

# 2. Actualizar dependencias
cargo update

# 3. Verificar Cargo.toml
cat Cargo.toml  # Verificar que todas las dependencias están presentes

# 4. Recompilar
cargo build --release
```

### Error: "PyO3 compilation failed"

**Síntomas:**
```
error: failed to run custom build command for `pyo3-ffi`
```

**Solución:**
```bash
# 1. Verificar versión de Python (debe ser 3.8+)
python --version

# 2. Reinstalar PyO3 dependencies
pip uninstall maturin
pip install --upgrade pip setuptools wheel
pip install maturin

# 3. Limpiar y recompilar
cargo clean
maturin develop --release
```

### Error: "linking with `cc` failed"

**Síntomas:**
```
error: linking with `cc` failed: exit status: 1
...
Undefined symbols for architecture arm64:
  "_PyBaseObject_Type", referenced from:
```

**Causa:**
Este error ocurre cuando se intenta compilar el proyecto con `cargo build` directamente. `cargo` no sabe dónde encontrar las librerías de Python para enlazarlas, por lo que falla.

**Solución:**
Usa `maturin` para compilar el proyecto. `maturin` se encarga de pasar las banderas correctas al compilador de Rust.

```bash
# Para desarrollo
maturin develop

# Para producción
maturin build --release
```

### Error: "cargo: permission denied"

**Síntomas:**
```bash
cargo: permission denied
```

**Solución:**
```bash
# Cambiar ownership del directorio
sudo chown -R $USER:$USER ~/.cargo
sudo chown -R $USER:$USER ./target

# O cambiar a directorio con permisos
cd ~/
git clone <repo-url> colmena_nuevo
cd colmena_nuevo
```

## 🏃 Problemas de Ejecución

### Error: "No module named 'colmena'"

**Síntomas:**
```python
ModuleNotFoundError: No module named 'colmena'
```

**Diagnóstico:**
```python
# Verificar instalación
import sys
print("Python path:")
for path in sys.path:
    print(f"  {path}")

# Verificar entorno virtual
import os
print(f"Virtual env: {os.environ.get('VIRTUAL_ENV', 'NONE')}")
```

**Solución:**
```bash
# 1. Verificar que el entorno virtual está activado
source venv/bin/activate

# 2. Verificar que maturin fue ejecutado
maturin develop --release

# 3. Verificar instalación
python -c "import colmena; print('OK')"

# 4. Si falla, reinstalar
pip uninstall colmena
maturin develop --release
```

### Error: "LlmException: Network error"

**Síntomas:**
```python
colmena.LlmException: Network error: connection failed
```

**Diagnóstico:**
```python
# Test de conectividad
import requests

apis = {
    "OpenAI": "https://api.openai.com/v1/models",
    "Gemini": "https://generativelanguage.googleapis.com/v1beta/models",
    "Anthropic": "https://api.anthropic.com/v1/messages"
}

for name, url in apis.items():
    try:
        response = requests.get(url, timeout=10)
        print(f"✅ {name}: Conectividad OK ({response.status_code})")
    except Exception as e:
        print(f"❌ {name}: Error - {e}")
```

**Solución:**
```bash
# 1. Verificar conexión a internet
ping google.com

# 2. Verificar proxy/firewall
curl -I https://api.openai.com/v1/models

# 3. Configurar proxy si es necesario
export HTTP_PROXY=http://proxy:port
export HTTPS_PROXY=http://proxy:port

# 4. Verificar DNS
nslookup api.openai.com
```

### Error: "Segmentation fault" al importar

**Síntomas:**
```bash
Segmentation fault (core dumped)
```

**Solución:**
```bash
# 1. Verificar arquitectura
uname -m
python -c "import platform; print(platform.machine())"

# 2. Recompilar para arquitectura específica
rustup target add x86_64-unknown-linux-gnu
cargo build --target x86_64-unknown-linux-gnu --release

# 3. Verificar versiones compatibles
python --version
rustc --version

# 4. Reinstalar desde cero
rm -rf venv/ target/
python3 -m venv venv
source venv/bin/activate
pip install maturin
cargo clean
maturin develop --release
```

## 🔑 Problemas con API Keys

### Error: "Invalid API key"

**Síntomas:**
```python
colmena.LlmException: Request failed: Invalid API key
```

**Diagnóstico:**
```python
# test_api_keys.py
import os
import requests

def test_openai_key(api_key):
    headers = {"Authorization": f"Bearer {api_key}"}
    response = requests.get("https://api.openai.com/v1/models", headers=headers)
    return response.status_code == 200

def test_gemini_key(api_key):
    url = f"https://generativelanguage.googleapis.com/v1beta/models?key={api_key}"
    response = requests.get(url)
    return response.status_code == 200

def test_anthropic_key(api_key):
    headers = {
        "x-api-key": api_key,
        "Content-Type": "application/json",
        "anthropic-version": "2023-06-01"
    }
    # Test con request mínimo
    data = {
        "model": "claude-3-sonnet-20240229",
        "max_tokens": 1,
        "messages": [{"role": "user", "content": "Hi"}]
    }
    response = requests.post("https://api.anthropic.com/v1/messages",
                           headers=headers, json=data)
    return response.status_code in [200, 400]  # 400 es OK (formato del request)

# Probar keys
keys = {
    "OpenAI": os.getenv("OPENAI_API_KEY", "tu-openai-key"),
    "Gemini": os.getenv("GEMINI_API_KEY", "tu-gemini-key"),
    "Anthropic": os.getenv("ANTHROPIC_API_KEY", "tu-anthropic-key")
}

tests = {
    "OpenAI": test_openai_key,
    "Gemini": test_gemini_key,
    "Anthropic": test_anthropic_key
}

for provider, key in keys.items():
    if key and key != f"tu-{provider.lower()}-key":
        result = tests[provider](key)
        status = "✅" if result else "❌"
        print(f"{status} {provider}: {'Válida' if result else 'Inválida'}")
    else:
        print(f"⚠️  {provider}: No configurada")
```

**Solución:**
1. **Verificar formato de API key:**
   - OpenAI: `sk-...` (comienza con sk-)
   - Gemini: `AIza...` (comienza con AIza)
   - Anthropic: `sk-ant-...` (comienza con sk-ant-)

2. **Regenerar API key:**
   - OpenAI: https://platform.openai.com/api-keys
   - Gemini: https://makersuite.google.com/app/apikey
   - Anthropic: https://console.anthropic.com/

3. **Verificar permisos y límites:**
   - Cuenta activa y con créditos
   - API key con permisos correctos
   - No exceder rate limits

### Error: "Rate limit exceeded"

**Síntomas:**
```python
colmena.LlmException: Request failed: Rate limit exceeded
```

**Solución con Retry Automático:**
```python
import time
import random

def call_with_retry(llm, messages, provider, api_key, max_retries=5):
    """Llamada con retry automático para rate limits"""

    for attempt in range(max_retries):
        try:
            return llm.call(messages=messages, provider=provider, api_key=api_key)

        except colmena.LlmException as e:
            if "rate limit" in str(e).lower() and attempt < max_retries - 1:
                # Backoff exponencial con jitter
                wait_time = (2 ** attempt) + random.uniform(0, 1)
                print(f"⏳ Rate limit alcanzado, esperando {wait_time:.1f}s...")
                time.sleep(wait_time)
                continue
            else:
                raise

# Uso
llm = colmena.ColmenaLlm()
response = call_with_retry(llm, ["Test"], "openai", "tu-api-key")
```

## 🐌 Problemas de Performance

### Llamadas muy lentas

**Diagnóstico:**
```python
import time
import colmena

def benchmark_call():
    llm = colmena.ColmenaLlm()

    # Test simple
    start = time.time()
    response = llm.call(
        messages=["Hi"],
        provider="google",
        api_key="tu-api-key"
    )
    total_time = time.time() - start

    print(f"⏱️  Tiempo total: {total_time:.2f}s")
    print(f"📊 Caracteres/segundo: {len(response) / total_time:.1f}")

    if total_time > 10:
        print("⚠️  Llamada muy lenta (>10s)")
    elif total_time > 5:
        print("⚠️  Llamada lenta (>5s)")
    else:
        print("✅ Velocidad normal")

benchmark_call()
```

**Soluciones:**
1. **Optimizar parámetros:**
   ```python
   # Reducir max_tokens para respuestas más rápidas
   response = llm.call(
       messages=["Respuesta corta por favor"],
       provider="google",
       max_tokens=100,  # Límite bajo
       api_key="tu-api-key"
   )
   ```

2. **Usar modelos más rápidos:**
   ```python
   # Modelos más rápidos por proveedor
   fast_models = {
       "openai": "gpt-3.5-turbo",      # Más rápido que gpt-4
       "google": "gemini-2.5-flash",   # Más rápido que gemini-pro
       "anthropic": "claude-3-haiku-20240307"  # Más rápido que sonnet
   }
   ```

3. **Implementar timeout:**
   ```python
   import signal

   def call_with_timeout(llm, messages, provider, api_key, timeout=30):
       def timeout_handler(signum, frame):
           raise TimeoutError("Llamada excedió el timeout")

       signal.signal(signal.SIGALRM, timeout_handler)
       signal.alarm(timeout)

       try:
           response = llm.call(messages=messages, provider=provider, api_key=api_key)
           signal.alarm(0)  # Cancelar timeout
           return response
       except TimeoutError:
           print(f"⏰ Timeout después de {timeout}s")
           raise
   ```

### Memoria excesiva

**Diagnóstico:**
```python
import psutil
import os

def monitor_memory():
    process = psutil.Process(os.getpid())

    # Memoria antes
    mem_before = process.memory_info().rss / 1024 / 1024  # MB
    print(f"🧠 Memoria antes: {mem_before:.1f} MB")

    # Crear múltiples instancias (test de leak)
    llms = []
    for i in range(10):
        llm = colmena.ColmenaLlm()
        llms.append(llm)

    # Memoria después
    mem_after = process.memory_info().rss / 1024 / 1024  # MB
    print(f"🧠 Memoria después: {mem_after:.1f} MB")
    print(f"📈 Incremento: {mem_after - mem_before:.1f} MB")

    # Limpiar
    del llms

monitor_memory()
```

**Soluciones:**
1. **Reutilizar instancias:**
   ```python
   # ❌ Malo: crear nueva instancia cada vez
   def bad_usage():
       llm = colmena.ColmenaLlm()  # Nueva instancia
       return llm.call(...)

   # ✅ Bueno: reutilizar instancia
   class MyApp:
       def __init__(self):
           self.llm = colmena.ColmenaLlm()  # Una sola instancia

       def process(self, messages):
           return self.llm.call(...)
   ```

2. **Implementar pool de conexiones:**
   ```python
   class LlmPool:
       def __init__(self, size=3):
           self.llms = [colmena.ColmenaLlm() for _ in range(size)]
           self.index = 0

       def get_llm(self):
           llm = self.llms[self.index]
           self.index = (self.index + 1) % len(self.llms)
           return llm

   # Uso
   pool = LlmPool()
   response = pool.get_llm().call(...)
   ```

## 🔍 Diagnóstico Avanzado

### Habilitar Debug Logging

```python
import logging
import colmena

# Configurar logging detallado
logging.basicConfig(
    level=logging.DEBUG,
    format='%(asctime)s - %(name)s - %(levelname)s - %(message)s'
)

# Activar logs de requests HTTP
import urllib3
urllib3.disable_warnings()
logging.getLogger("urllib3").setLevel(logging.DEBUG)

# Ahora las llamadas mostrarán información detallada
llm = colmena.ColmenaLlm()
response = llm.call(["Debug test"], "google", api_key="tu-api-key")
```

### Verificar Bindings Nativos

```python
import inspect
import colmena

def verify_native_bindings():
    """Verificar que estamos usando código Rust nativo"""

    llm = colmena.ColmenaLlm()

    # Verificar tipo de métodos
    print(f"Tipo de ColmenaLlm: {type(llm)}")
    print(f"Tipo de método call: {type(llm.call)}")
    print(f"Tipo de método stream: {type(llm.stream)}")

    # Intentar obtener código fuente (debería fallar para código nativo)
    try:
        source = inspect.getsource(llm.call)
        print("❌ WARNING: Método call() tiene código Python visible")
        print(source)
    except (OSError, TypeError) as e:
        print(f"✅ Método call() es nativo: {type(e).__name__}")

    # Verificar módulo compilado
    print(f"Archivo del módulo: {colmena.__file__}")
    if colmena.__file__.endswith('.so') or colmena.__file__.endswith('.pyd'):
        print("✅ Módulo compilado detectado")
    else:
        print("⚠️  Módulo no parece estar compilado")

verify_native_bindings()
```

### Test de Stress

```python
import threading
import time
import colmena

def stress_test():
    """Test de stress para detectar problemas de concurrencia"""

    llm = colmena.ColmenaLlm()
    errors = []
    responses = []

    def worker(thread_id):
        try:
            response = llm.call(
                messages=[f"Thread {thread_id} test"],
                provider="google",
                api_key="tu-api-key"
            )
            responses.append(f"Thread {thread_id}: OK")
        except Exception as e:
            errors.append(f"Thread {thread_id}: {e}")

    # Crear múltiples threads
    threads = []
    for i in range(5):
        thread = threading.Thread(target=worker, args=(i,))
        threads.append(thread)

    # Ejecutar en paralelo
    start_time = time.time()
    for thread in threads:
        thread.start()

    for thread in threads:
        thread.join()

    total_time = time.time() - start_time

    print(f"⏱️  Tiempo total: {total_time:.2f}s")
    print(f"✅ Éxitos: {len(responses)}")
    print(f"❌ Errores: {len(errors)}")

    if errors:
        print("\nErrores encontrados:")
        for error in errors:
            print(f"  {error}")

# stress_test()  # Descomenta para ejecutar
```

## 📞 Obtener Ayuda

### Información para Reportar Bugs

Cuando reportes un problema, incluye:

```python
# bug_report.py
import sys
import platform
import colmena

def generate_bug_report():
    """Generar reporte completo para debugging"""

    print("🐛 REPORTE DE BUG - COLMENA")
    print("=" * 50)

    # Información del sistema
    print("🖥️  SISTEMA:")
    print(f"   OS: {platform.system()} {platform.release()}")
    print(f"   Arquitectura: {platform.machine()}")
    print(f"   Python: {sys.version}")

    # Información de Colmena
    print("\n🐝 COLMENA:")
    try:
        print(f"   Archivo: {colmena.__file__}")
        print(f"   Tipo: {type(colmena.ColmenaLlm())}")

        # Métodos disponibles
        llm = colmena.ColmenaLlm()
        methods = [m for m in dir(llm) if not m.startswith('_')]
        print(f"   Métodos: {methods}")

    except Exception as e:
        print(f"   Error: {e}")

    # Dependencias
    print("\n📦 DEPENDENCIAS:")
    deps = ["maturin", "requests", "setuptools"]
    for dep in deps:
        try:
            module = __import__(dep)
            version = getattr(module, '__version__', 'unknown')
            print(f"   {dep}: {version}")
        except ImportError:
            print(f"   {dep}: NO INSTALADO")

    print("\n" + "=" * 50)
    print("📧 Incluye esta información al reportar el bug")

generate_bug_report()
```

### Canales de Soporte

1. **Documentación**: Lee primero esta guía completa
2. **Issues GitHub**: Reporta bugs específicos con información completa
3. **Discusiones**: Para preguntas generales y casos de uso
4. **Email**: Solo para problemas críticos de seguridad

### Template para Issues

```markdown
## 🐛 Descripción del Problema

Descripción clara del problema...

## 🔄 Pasos para Reproducir

1. Paso 1...
2. Paso 2...
3. Error aparece...

## 💻 Información del Sistema

- OS: [Ubuntu 20.04 / macOS 12 / Windows 11]
- Python: [3.9.7]
- Rust: [1.75.0]
- Colmena: [versión]

## 📋 Código que Falla

```python
# Código mínimo que reproduce el error
import colmena
llm = colmena.ColmenaLlm()
# ... resto del código
```

## 📄 Output/Error Completo

```
Error completo aquí...
```

## 🔍 Información Adicional

Cualquier información adicional relevante...
```

---

**🐝 Colmena** - *Solucionando problemas juntos*

> 💡 **Tip**: La mayoría de problemas se resuelven limpiando caché (`cargo clean`) y recompilando (`maturin develop --release`)

---

## 🗄️ Problemas con Migraciones de Base de Datos

### Error: "migration was previously applied but is missing"

**Síntomas:**
```
error: while executing migrations: migration 20260302000000 was previously applied but is missing in the resolved migrations
```

**Causa:** La tabla `_sqlx_migrations` en PostgreSQL registra cada migración aplicada con su checksum. Si se consolidan archivos de migración (por ejemplo, se unifican varios `.sql` en uno solo), los registros viejos quedan huérfanos.

**Solución:**
```bash
# Eliminar la tabla de tracking de migraciones
psql $DATABASE_URL -c "DROP TABLE IF EXISTS _sqlx_migrations;"

# Re-ejecutar el grafo — las migraciones se aplicarán de nuevo
cargo run --bin dag_engine -- run <path/to/graph.json>
```

**Nota:** Las tablas de datos usan `CREATE TABLE IF NOT EXISTS`, por lo que no se pierden datos al limpiar `_sqlx_migrations`. Además, el migrador tiene `set_ignore_missing(true)` como protección para futuras consolidaciones.

---

### Error: "migration was previously applied but has been modified"

**Síntomas:**
```
error: while executing migrations: migration 20260302000000 was previously applied but has been modified
```

**Causa:** El contenido del archivo de migración cambió respecto a lo que fue aplicado originalmente (checksum diferente).

**Solución:** Misma que arriba — eliminar `_sqlx_migrations` y re-ejecutar.

---

## 🤖 Problemas con Gemini Streaming y Tool Calling

### Error: "Failed to parse arguments for tool: trailing characters"

**Síntomas:**
```
Failed to parse arguments for tool search_products: trailing characters at line 1 column 74
```

**Causa:** Cuando Gemini devolvía múltiples tool calls en paralelo (ej. `get_categories` y `search_products`), el adapter de streaming usaba `index: 0` para todos los chunks. El acumulador en `agent_service.rs` concatenaba los argumentos JSON de todas las herramientas en una sola entrada, produciendo JSON malformado como: `{"select":"...","q":"smartphone"}{}`.

**Fix aplicado (2026-04-08):** El adapter de Gemini (`gemini_adapter.rs`) ahora usa un contador `tool_call_index` incremental para asignar un índice único a cada tool call chunk en streaming. Cada herramienta se acumula por separado.

**Verificación:** Si Gemini llama múltiples herramientas en paralelo, cada una debe parsearse correctamente sin errores de "trailing characters".

**Archivo relevante:** `src/libs/colmena/src/llm/infrastructure/gemini_adapter.rs`

---

### `Consecutive messages with the same role` (resuelto 2026-06-20)
Antes, un turno que fallaba tras persistir el mensaje del usuario dejaba un
`user` colgado y trababa la conversación entera. `LlmRequest::new` ahora fusiona
roles consecutivos (coalescing) y auto-cura. Si lo ves en una versión vieja:
actualizá colmena o abrí un chat nuevo. Ver CHANGELOG §42.

---

## 🔒 Problemas con Secure Values en HTTP Tools

### HTTP tool con `secure: true` devuelve el token real al LLM

**Síntomas:** El LLM llama una tool de autenticación y en el `tool-output-available` aparece el token real:
```json
{"output": {"body": {"access_token": "859Tnd9E7SBSCbnO9E4..."}}}
```

**Causa:** `DagToolExecutor` no aplicaba `hash_output()` después de ejecutar tools — solo el DAG normal lo hacía.

**Fix aplicado (2026-04-05):** `DagToolExecutor.execute()` ahora detecta `"secure": true` en el `fixed_config` y llama `hash_output()` antes de devolver el resultado al LLM. El LLM recibe `<value_1>` en lugar del token real.

**Verificación:** Busca en el output:
```
🔒 [DagToolExecutor] Secure tool 'get_amadeus_token': output hashed, real values encrypted in DB
```

**Alerta si ves:**
```
⚠️ [DagToolExecutor] Tool 'X' has secure:true but no SecureValueService attached. Token WILL be visible to LLM.
```
→ El engine no tiene `DATABASE_URL` o `SECURE_VALUES_KEY` configurados.

---

### HTTP tool con `secure: true` retorna 400 a la API externa

**Síntomas:** La llamada a OAuth2 / token endpoint retorna 400 sin motivo aparente (las credenciales son correctas).

**Causa raíz:** El campo `"secure": true` del `fixed_config` se estaba enviando como query param a la API externa (`?secure=true`), ya que `"secure"` no estaba en la lista `reserved_keys` del `HttpNode`.

**Fix aplicado (2026-04-05):** `"secure"` fue añadido a `reserved_keys` en `HttpNode`. Nunca se enviará a APIs externas.

También se encontró y corrigió el typo: `"query_parameters"` → `"query_params"` (la clave correcta en el codebase).

**Si encuentras un campo interno que se filtra como query param**, añádelo a `reserved_keys` en:
```
src/libs/colmena/src/dag_engine/infrastructure/nodes/http.rs
```

---

### El output de debug expone tokens y credenciales en logs

**Síntomas:** En el stdout aparecen líneas como:
```
DEBUG: Response Body: {"access_token": "real_token_xyz", ...}
DEBUG: Request Body: grant_type=client_credentials&client_secret=...
```

**Causa:** `HttpNode` tenía `println!` que imprimían el body de request y response completo.

**Fix aplicado (2026-04-05):** Todos los `println!` de request/response body fueron eliminados. Los logs ahora solo muestran método, URL y status code:
```
[HttpNode] → POST https://api.amadeus.com/v1/security/oauth2/token
[HttpNode] ← 200 (https://api.amadeus.com/v1/security/oauth2/token)
```

**Regla general:** Nunca loguear `body` en `HttpNode` — puede contener tokens, claves API o PII.

---

### Cómo ejecutar el experimento de autenticación LLM + Amadeus

```bash
# Configurar variables (usando set -a para exportar a subprocesos)
set -a && source .env && set +a

# Ejecutar el grafo de experimento
cargo run --bin dag_engine -- run tests/graphs/agents/amadeus_llm_http_auth_experiment.json
```

**Qué buscar en el output seguro:**
```
[HttpNode] → POST https://api.amadeus.com/v1/security/oauth2/token
[HttpNode] ← 200 (...)
🔒 [DagToolExecutor] Secure tool 'get_amadeus_token': output hashed, real values encrypted in DB
tool-output → {"body": {"access_token": "<value_1>", ...}}   ← ✅ placeholder, no token real

[HttpNode] → GET https://api.amadeus.com/v2/shopping/flight-offers
[HttpNode] ← 200 (...)   ← ✅ bearer real inyectado, no visible en logs
```

**Variables requeridas:**
| Variable | Propósito |
|---|---|
| `AMADEUS_CLIENT_ID` | ID de credencial Amadeus (se resuelve por `HttpNode.resolve_env_vars`) |
| `AMADEUS_CLIENT_SECRET` | Secret de credencial Amadeus |
| `OPENAI_API_KEY` | API key del LLM |
| `DATABASE_URL` | PostgreSQL para almacenar `<value_N>` → token encriptado |
| `SECURE_VALUES_KEY` | Clave de pgcrypto (`pgp_sym_encrypt`) para encriptar en DB (mínimo 32 chars) |