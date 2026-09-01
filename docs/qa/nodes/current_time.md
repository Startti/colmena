# QA — Nodo `current_time`

Fuente de código: `src/libs/colmena/src/dag_engine/infrastructure/nodes/current_time.rs`
Fuentes de doc revisadas: `docs/node_configurations.json`, `docs/agent_context/node_ports_reference.md`, `docs/developer_guide/29_lazy_tool_loading.md`, `docs/developer_guide/40_toolkit_packages.md`

## 1) Config documentada NO soportada por el código

Sin discrepancias detectadas. La documentación describe correctamente que el nodo no toma configuración (`config_fields: {}`), no lee inputs (`input_ports: {}`), y genera la timestamp fresca en cada ejecución sin depender de parámetros externos.

## 2) Código NO documentado

**Hallazgo 1:** El nodo `current_time` no aparece registrado en `docs/node_as_tools_reference.json`.
- **Descripción:** Aunque la documentación en `node_configurations.json` menciona que el nodo es "comúnmente usado como tool built-in del LLM" y las guías de `developer_guide/29_lazy_tool_loading.md` y `40_toolkit_packages.md` lo usan como ejemplo de herramienta eager/automática, no existe una entrada en el archivo `node_as_tools_reference.json` que documente formalmente cómo se expone como tool o su esquema de tool.
- **Impacto:** Un desarrollador que intenta entender la configuración de `current_time` en `tool_configurations` no encontrará la referencia canónica en `node_as_tools_reference.json`.

**Hallazgo 2:** El método `default_output()` retorna explícitamente `"output"` (línea 29 de current_time.rs), pero la descripción general del nodo no menciona este puerto por defecto explícitamente.
- **Descripción:** El código define que el puerto de salida por defecto es `"output"`, documentado en `node_configurations.json` con `"default_output": "output"`, pero la descripción larga del nodo no subraya que cuando el nodo se usa sin especificar un puerto de salida, automáticamente selecciona `output`.
- **Impacto:** Bajo. La documentación es correcta pero podría ser más explícita sobre este comportamiento en la descripción del nodo.

**Hallazgo 3:** Discrepancia terminológica entre descripción del código y documentación.
- **Descripción:** El método `description()` en el código (línea 40-41) dice "Return the current UTC timestamp as an **ISO-8601 string**", mientras que el código implementa `Utc::now().to_rfc3339()`, que genera **RFC3339**. El documento `node_configurations.json` es más preciso: dice "**RFC3339 / ISO-8601 string**". RFC3339 es un perfil estricto de ISO-8601, no son sinónimos exactos.
- **Impacto:** Bajo. Ambos formatos son compatibles; sin embargo, la descripción en el código debería decir "RFC3339" para ser precisa, o "RFC3339 (ISO-8601)" para ser consistente con la documentación.

## 3) Plan de pruebas QA

### Caso 1: Happy path — ejecutar sin config ni inputs

**Objetivo:** Verificar que el nodo ejecuta correctamente y retorna una timestamp válida en el puerto `output`.

**Grafo JSON mínimo:**
```json
{
  "nodes": [
    {
      "id": "get_time",
      "node_type": "current_time",
      "config": {}
    }
  ],
  "edges": [],
  "entry_point": "get_time"
}
```

**Comando:**
```bash
cargo run --bin dag_engine -- run tests/graphs/qa/current_time_basic.json
```

**Entrada:** Ninguna.

**Resultado esperado:**
- El nodo retorna `{ "output": "<timestamp-rfc3339>" }`.
- El SSE contiene un evento `node-finished` con `output: { "output": "<timestamp>" }`.
- El campo `output` es un string en formato RFC3339 (ej. `"2026-08-30T12:34:56Z"` o `"2026-08-30T12:34:56+00:00"`).

**Verificación:** 
- Parsear el JSON de salida y verificar que `output.output` es un string.
- Validar que contiene un carácter 'T' (separador de fecha/hora en RFC3339).
- Validar que termina con 'Z' (UTC) o contiene un offset de timezone (ej. `+00:00` o `-05:00`).

---

### Caso 2: Formato RFC3339 válido

**Objetivo:** Verificar que la timestamp retornada es un RFC3339 completo y válido en UTC.

**Grafo JSON mínimo:** (mismo que Caso 1)

**Comando:**
```bash
cargo run --bin dag_engine -- run tests/graphs/qa/current_time_basic.json | grep -o '"output":"[^"]*"'
```

**Entrada:** Ninguna.

**Resultado esperado:**
- La timestamp tiene la estructura: `YYYY-MM-DDTHH:MM:SS[.fff][±HH:MM|Z]`.
- Ejemplos válidos: `"2026-07-24T15:30:00Z"`, `"2026-07-24T15:30:00+00:00"`.
- La fecha y hora son numéricamente válidas (mes 1-12, día 1-31, hora 0-23, etc.).

**Verificación:**
- Usar `chrono::DateTime::parse_from_rfc3339()` para parsear la timestamp en una herramienta de test (ej. Python con `datetime.fromisoformat()`).
- Confirmar que el parsing no falla.

---

### Caso 3: Ignora config y inputs

**Objetivo:** Verificar que el nodo genera correctamente la timestamp incluso si se proporcionan config o inputs inesperados (debe ignorarlos).

**Grafo JSON con config/inputs no estándar:**
```json
{
  "nodes": [
    {
      "id": "get_time",
      "node_type": "current_time",
      "config": {
        "extra_field": "should_be_ignored",
        "timezone": "America/New_York"
      },
      "inputs": {
        "ignored_input": "previous_output"
      }
    }
  ],
  "edges": [],
  "entry_point": "get_time"
}
```

**Comando:**
```bash
cargo run --bin dag_engine -- run tests/graphs/qa/current_time_with_ignored_config.json
```

**Entrada:** Ninguna.

**Resultado esperado:**
- El nodo ejecuta sin error.
- Retorna `{ "output": "<timestamp-utc>" }`.
- La timestamp está en UTC (termina en 'Z' o `+00:00`), NO en otra timezone (como `-05:00` para Nueva York).

**Verificación:**
- Confirmar que el nodo no falla por campos de config desconocidos.
- Extraer la timestamp y confirmar que el offset es `+00:00` o `Z`.

---

### Caso 4: Ejecución múltiple genera timestamps distintas

**Objetivo:** Verificar que cada invocación genera una nueva timestamp (no cacheada), y que son progresivamente posteriores.

**Script Python:**
```python
import subprocess
import json
import time
from datetime import datetime

timestamps = []
for i in range(3):
    result = subprocess.run(
        ["cargo", "run", "--bin", "dag_engine", "--", "run", "tests/graphs/qa/current_time_basic.json"],
        capture_output=True,
        text=True
    )
    # Extraer output del último evento node-finished
    for line in result.stdout.split('\n'):
        if '"node-finished"' in line:
            event = json.loads(line)
            ts = event.get('output', {}).get('output')
            if ts:
                timestamps.append(ts)
                break
    if i < 2:
        time.sleep(0.1)  # Pequeña pausa entre invocaciones

print(f"Timestamps: {timestamps}")
assert len(timestamps) == 3, f"Expected 3 timestamps, got {len(timestamps)}"
assert timestamps[0] != timestamps[1], "Timestamps should be different"
# Validar orden cronológico (parsear RFC3339 y comparar)
for i in range(len(timestamps) - 1):
    t1 = datetime.fromisoformat(timestamps[i].replace('Z', '+00:00'))
    t2 = datetime.fromisoformat(timestamps[i+1].replace('Z', '+00:00'))
    assert t1 < t2, f"Timestamp {i} should be < timestamp {i+1}"
print("✓ All timestamps are unique and chronologically ordered")
```

**Comando:**
```bash
python3 tests/qa/current_time_multiple_runs.py
```

**Entrada:** Ninguna.

**Resultado esperado:**
- Se capturan 3 timestamps distintas.
- Las timestamps están en orden cronológico (t1 < t2 < t3).
- No hay caching: cada ejecución genera una nueva timestamp basada en `Utc::now()`.

**Verificación:**
- Confirmar que no hay dos timestamps iguales.
- Parsear cada timestamp y comparar datetime objects.
- Confirmar que el tiempo transcurrido entre invocaciones refleja las pausas de 0.1s.

---

### Caso 5: Timezone es siempre UTC

**Objetivo:** Verificar que la timestamp retornada siempre está en UTC, independientemente de la timezone del sistema.

**Grafo JSON mínimo:** (mismo que Caso 1)

**Comando (con timezone simulada):**
```bash
TZ=America/Los_Angeles cargo run --bin dag_engine -- run tests/graphs/qa/current_time_basic.json
TZ=Asia/Tokyo cargo run --bin dag_engine -- run tests/graphs/qa/current_time_basic.json
```

**Entrada:** Ninguna.

**Resultado esperado:**
- En ambos casos, la timestamp termina con `Z` o `+00:00`, indicando UTC.
- La hora exacta es idéntica en ambas ejecuciones (no depende de `TZ`).

**Verificación:**
- Extraer la timestamp de ambas ejecuciones.
- Parsear ambas y confirmar que representan el mismo instante UTC.
- Confirmar que el offset es siempre `+00:00` o `Z`.

---

### Caso 6: Como tool del LLM (enabled_tools)

**Objetivo:** Verificar que `current_time` funciona correctamente cuando se expone como herramienta built-in del `llm_call`.

**Grafo JSON:**
```json
{
  "nodes": [
    {
      "id": "agent",
      "node_type": "llm_call",
      "config": {
        "provider": "anthropic",
        "model": "claude-3-5-sonnet-20241022",
        "prompt": "What is the current time?",
        "system_message": "You are a helpful assistant. Use the current_time tool to answer the user's question.",
        "enabled_tools": ["current_time"]
      }
    }
  ],
  "edges": [],
  "entry_point": "agent"
}
```

**Comando:**
```bash
ANTHROPIC_API_KEY=sk-... cargo run --bin dag_engine -- run tests/graphs/qa/current_time_as_tool.json
```

**Entrada:** Ninguna (o `--agent-session-id qa_time_001` si se persiste).

**Resultado esperado:**
- El `llm_call` inicia con `current_time` disponible en su herramientas.
- El modelo llama a `current_time()` (sin parámetros).
- Recibe la respuesta `{ "output": "<timestamp>" }`.
- El modelo procesa la timestamp y responde con la hora actual.

**Verificación:**
- Capturar SSE y buscar `tool-call-started` con `tool_name: "current_time"`.
- Confirmar que el resultado del tool (en el siguiente evento) contiene un string RFC3339 válido.
- Confirmar que el modelo usa esa timestamp en su respuesta final.

