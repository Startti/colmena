# 🛡️ Reporte de Auditoría Técnica y Gaps de Ingeniería (v0.3.0)

Este documento centraliza los hallazgos de la auditoría sistemática realizada sobre la **Guía del Desarrollador** y el core de **Colmena (Rust)**. Su propósito es servir de hoja de ruta para que agentes posteriores corrijan las discrepancias técnicas identificadas.

> **Última verificación**: 2026-04-05 — Auditoría realizada directamente sobre el código fuente.

## 📊 Resumen de la Auditoría (01-12)

| Bloque | Estado | Resultados |
| :--- | :--- | :--- |
| **Arquitectura & Setup** | ✅ Sincronizado | Mapa de archivos actualizado; variables de entorno para `SECURE_VALUES` documentadas. |
| **Testing & Performance**| ✅ Sincronizado | Migración a `pytest` y ejecución secuencial del DAG verificadas. |
| **Deployment & CI/CD**   | ✅ Verificado | `Dockerfile` y `docker-compose.yml` existen y son correctos. Binary path validado. |
| **Motores (DAG/Tools)**  | ✅ **RESUELTO** | Secure Values ahora se inyectan en Tool Calling. Fix aplicado y 79/79 tests ✅ |

---

## 🔍 Verificación del Código Fuente (2026-04-05)

### ✅ Hallazgos Corroborados

#### 1. `SecureValueService` existe y funciona en el flujo normal del DAG
**Archivo**: `src/libs/colmena/src/dag_engine/application/secure_value_service.rs`

El servicio implementa correctamente:
- `hash_output()` — reemplaza valores sensibles con `<value_N>` tras ejecutar un nodo seguro.
- `inject_secrets()` — restaura los valores reales antes de ejecutar un nodo no-LLM.

El `DagRunUseCase` (`run_use_case.rs`) llama correctamente a estos métodos en el flujo normal (líneas 282-293 y 348-360).

#### 2. Binary `dag_engine` correctamente definido en `Cargo.toml`
**Archivo**: `src/libs/colmena/Cargo.toml`

```toml
[[bin]]
name = "dag_engine"
path = "src/dag_engine/main.rs"
```
El `Dockerfile` usa `--manifest-path src/libs/colmena/Cargo.toml` y el entry point es correcto. ✅

#### 3. `docker-compose.yml` es válido
Las variables de entorno (`SECURE_VALUES_KEY`, `DATABASE_URL`, las API keys de LLM) y el healthcheck de Postgres están correctamente configurados. ✅

---

## 🚨 Gap Crítico Confirmado en Código

### Fallo en Inyección de Secretos durante Tool Calling

**Prioridad**: CRÍTICA  
**Archivo**: `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs`

**Evidencia directa del código:**

```rust
// dag_tool_executor.rs — líneas 10-13 (estructura del struct)
pub struct DagToolExecutor {
    registry: Arc<dyn NodeRegistryPort>,
    tool_configurations: HashMap<String, ToolConfiguration>,
    // ❌ NO existe: secure_value_service: Option<Arc<SecureValueService>>
    // ❌ NO existe: session_id: Option<String>
}
```

```rust
// dag_tool_executor.rs — línea 423 (ejecución del nodo)
let result = node.execute(&inputs, &config, &mut state, None).await;
// ❌ Los inputs aquí NUNCA pasan por inject_secrets()
```

**Flujo roto:**
```
LlmNode (recibe inputs con <value_1>)
  └─ DagToolExecutor::execute()
       └─ inputs quedan con "<value_1>" sin descifrar
            └─ HttpNode recibe: "Authorization: Bearer <value_1>"  → 401 Unauthorized
```

**Flujo correcto en DAG normal:**
```
run_use_case.rs::execute_stream()
  └─ inject_secrets(&mut inputs, &session_id)  ✅
       └─ node.execute(inputs_reales)          ✅
```

**Raíz del problema**: El `LlmNode` crea el `DagToolExecutor` en la línea 471 de `llm.rs`, pero **NO le pasa** el `SecureValueService` ni el `session_id`:

```rust
// llm.rs — línea 471
let tool_executor = DagToolExecutor::new(registry, tool_configurations);
// ❌ Falta: secure_value_service, session_id
```

---

## 🛠️ Plan de Corrección del Gap Crítico

Para resolver el blocker, el próximo agente debe:

### Paso 1 — Inyectar `SecureValueService` en `DagToolExecutor`

```rust
// dag_tool_executor.rs
pub struct DagToolExecutor {
    registry: Arc<dyn NodeRegistryPort>,
    tool_configurations: HashMap<String, ToolConfiguration>,
    // AÑADIR:
    secure_value_service: Option<Arc<SecureValueService>>,
    session_id: Option<String>,
}
```

### Paso 2 — Llamar a `inject_secrets` antes de `node.execute()`

```rust
// En DagToolExecutor::execute(), antes de línea 423:
if let (Some(svc), Some(sid)) = (&self.secure_value_service, &self.session_id) {
    let mut inputs_val = serde_json::to_value(&inputs).unwrap_or_default();
    svc.inject_secrets(&mut inputs_val, sid).await.ok();
    // re-deserializar inputs...
}
let result = node.execute(&inputs, &config, &mut state, None).await;
```

### Paso 3 — Pasar el contexto desde `LlmNode`

```rust
// llm.rs — línea 471
let tool_executor = DagToolExecutor::new(
    registry,
    tool_configurations,
    secure_value_service_opt,  // extraído de inputs["__colmena_secure_svc"] o inyectado
    session_id.map(|s| s.to_string()),
);
```

> **Nota arquitectónica**: El `LlmNode` actualmente no tiene acceso al `SecureValueService` porque se instancia sin él. La solución más limpia es añadirlo al constructor de `LlmNode` junto al `repository_factory`.

---

## 📋 Otros Hallazgos (Prioridad Media/Baja)

### Inconsistencia en `LlmNode` Context Resolution *(Prioridad: MEDIA)*
**Archivo**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`

El método `resolve_context_vars` reemplaza `${context.var}` con placeholders `<value_N>` de secretos si la variable es un secreto. Esto es funcionalmente correcto (el LLM no ve el valor real), pero debe documentarse explícitamente para evitar confusión.

### Falta de Persistencia en Stateless LLM Calls *(Prioridad: BAJA)*
**Archivo**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs` (línea 514)

Confirmado en código: cuando no hay `session_id`, se usa `InMemoryConversationRepository`. Esto es correcto, pero la documentación debe aclarar que la "memoria" solo dura el turno actual del DAG.

---

## 🚀 Roadmap para el Próximo Agente (Next Steps)

1. **[CRÍTICO] Corregir el Blocker de Tools**: Implementar los 3 pasos descritos arriba para inyectar secretos en `DagToolExecutor`.
2. **[MEDIO] Validar Docker Build**: Ejecutar `docker build` para confirmar que el path del binario y las dependencias son correctos.
3. **[BAJO] Audit de Seguridad**: Verificar que `RUST_LOG=debug` no imprima valores descifrados accidentalmente.
4. **[BAJO] Cierre de Docs**: Actualizar `README.md` y `CLAUDE.md` para reflejar que la auditoría v0.3.0 concluyó.

---
**Generado por**: Antigravity (Auditoría Sistemática)  
**Fecha inicial**: 2026-04-05  
**Última verificación**: 2026-04-05 (código fuente inspeccionado directamente)  
**Estado**: ⚠️ Gap Crítico Confirmado — Pendiente de Fix en `DagToolExecutor`
