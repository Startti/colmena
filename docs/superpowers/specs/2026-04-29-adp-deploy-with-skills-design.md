# ADP Deploy con Skills Externalizadas — Design

**Fecha:** 2026-04-29
**Topic:** Desplegar los últimos cambios de Colmena al worker de ADP en Cloud Run, moviendo las skills ADP-specific desde el repo de Colmena al repo de ADP.

## Contexto

El worker de ADP (`apps/service/ia/platform/worker`) consume Colmena como dependencia git desde `https://github.com/Startti/colmena` rama `develop`. El deploy se ejecuta vía `apps/service/ia/platform/deploy_gcp.sh`, que arma dos imágenes (`colmena-api`, `colmena-worker`) y las despliega a Cloud Run.

Hay dos cosas que necesitan llegar a producción:

1. Cambios pendientes en Colmena local: GCS storage backend, sandboxing del Python node, `DocumentRuntime` async init, ajustes en `llm.rs`, `document_nodes.rs`, y los JSON canónicos de `docs/`.
2. Skills ADP-specific (catálogo de tipos de nodo del canvas) que hoy viven sueltas en `colmena/tests/graphs/external/skills/adp-node-catalog/` (untracked). Estas skills NO pertenecen a Colmena — son del producto ADP, no de la librería de orquestación.

El requerimiento es: skills viven exclusivamente en ADP, dentro del crate `worker`, y el contenedor del worker las monta en una ruta estable que el motor de Colmena pueda leer en runtime.

## Cómo carga skills el LLM node

El nodo `llm_call` de Colmena resuelve `skills.paths` así (ver [llm.rs:89-138](../../../src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs)):

1. `graph_dir` = directorio que contiene el JSON del grafo si está disponible vía input `__colmena_graph_path`; si no, **`std::env::current_dir()`**.
2. Cada path en `skills.paths` se resuelve relativo a `graph_dir` (si es relativo) o se usa tal cual (si es absoluto), luego se `canonicalize()`.
3. El path canonicalizado debe estar dentro de `graph_dir` o de algún directorio listado en `COLMENA_SKILLS_ALLOWED_DIRS` (separador `:` en Unix).

En el worker, los grafos llegan como JSON en mensajes de Redis (ver [worker/src/main.rs:170](../../../../adp/apps/service/ia/platform/worker/src/main.rs)), nunca como archivo en disco — por lo tanto `__colmena_graph_path` no se inyecta y `graph_dir` cae al CWD del contenedor (`/app`, definido por `WORKDIR /app` en el Dockerfile).

## Decisiones

### D1. Ubicación de skills en ADP

`apps/service/ia/platform/worker/skills/`

Las skills viven dentro del crate `worker`, no en `platform/` ni en `shared/`. El worker es el único componente que ejecuta grafos; ningún otro consumer las necesita hoy.

### D2. Estrategia de paths en grafos

Path absoluto + `COLMENA_SKILLS_ALLOWED_DIRS`.

- En el contenedor: skills viven en `/app/skills/`.
- Grafos en producción referencian `"paths": ["/app/skills/<skill-name>"]`.
- `deploy_gcp.sh` exporta `COLMENA_SKILLS_ALLOWED_DIRS=/app/skills` al servicio Cloud Run del worker.

Justificación: el path absoluto deja el contrato explícito ("el worker monta sus skills en `/app/skills/`") y el env var en el deploy script hace visible que ese directorio es load-bearing. Un path relativo funcionaría hoy (porque `graph_dir = /app` y `/app/skills` está dentro), pero el contrato sería implícito y dependería del WORKDIR del Dockerfile.

### D3. Qué se queda en Colmena

- Borrar `tests/graphs/external/skills/` por completo del repo de Colmena.
- El grafo de test `tests/graphs/external/socketio_canvas_builder.json` se queda en Colmena con el path absoluto a la ubicación de ADP (`/home/daniel-garcia4/startti/adp/apps/service/ia/platform/worker/skills/adp-node-catalog`). Es un artefacto de testing local del usuario; para correrlo se requiere `export COLMENA_SKILLS_ALLOWED_DIRS=/home/daniel-garcia4/startti/adp/apps/service/ia/platform/worker/skills`.
- En producción, los grafos que ejecuta el worker no son `socketio_canvas_builder.json` — son grafos generados por la UI/canvas de ADP y persistidos vía API. Esos grafos referencian `/app/skills/...`. El grafo de test en Colmena solo sirve para iteración local.

### D4. Cambios pendientes de Colmena

Hay 9 archivos modificados y 4 untracked en Colmena local. Los relevantes para producción:

- **Modificados** (push a develop):
  - `src/libs/colmena/src/dag_engine/infrastructure/nodes/document_nodes.rs`
  - `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`
  - `src/libs/colmena/src/dag_engine/infrastructure/nodes/python_node.rs`
  - `src/libs/colmena/src/documents/application/runtime.rs`
  - `src/libs/colmena/src/documents/infrastructure/storage/mod.rs`
  - `docs/node_as_tools_reference.json`, `docs/node_configurations.json`
- **Untracked** (push a develop):
  - `src/libs/colmena/src/documents/infrastructure/storage/gcs_store.rs`
- **No se commitean a Colmena**:
  - `tests/graphs/external/skills/` → se mueve a ADP, después se borra de Colmena.
  - `tests/graphs/external/socketio_canvas_builder.json` → se queda untracked (archivo de testing personal con path absoluto al home del usuario).
  - `tests/graphs/agents/python_llm_graph.json`, `tests/graphs/external/socketio_canvas_test.json`, `tests/graphs/advanced/nested_orchestrators_with_tools.json` → revisar uno por uno; si son tests del usuario, dejar untracked. Si son tests reusables (sin paths absolutos a su home), commitear.

## Arquitectura del cambio

### Layout de directorios

```
adp/apps/service/ia/platform/
├── worker/
│   ├── Cargo.toml
│   ├── Dockerfile           ← +1 línea COPY
│   ├── skills/              ← NUEVO
│   │   └── adp-node-catalog/
│   │       ├── SKILL.md
│   │       └── references/
│   │           ├── agent.md
│   │           ├── apiCall.md
│   │           ├── chatInput.md
│   │           ├── chatOutput.md
│   │           ├── databaseQuery.md
│   │           ├── llmCall.md
│   │           └── webSearch.md
│   └── src/main.rs
├── api/
├── shared/
├── cloudbuild.yaml
├── deploy_gcp.sh             ← +1 entry en build_env_vars()
└── ...
```

### Flujo en runtime

```
Repo ADP (host)                  Build context              Worker container
─────────────────                ─────────────              ────────────────
worker/skills/                   worker/skills/             /app/skills/
  adp-node-catalog/    ──COPY──>   adp-node-catalog/  ──>     adp-node-catalog/
    SKILL.md                                                    SKILL.md
    references/*.md                                             references/*.md

env vars en Cloud Run del worker:
  COLMENA_SKILLS_ALLOWED_DIRS=/app/skills

Grafo recibido por Redis:
  { "skills": { "paths": ["/app/skills/adp-node-catalog"] }, ... }

Validación en el LLM node:
  graph_dir = /app                                  (CWD, fallback)
  paths[0] absolute → canonicalize → /app/skills/adp-node-catalog
  allowed = [/app, /app/skills]                     (graph_dir + env var)
  /app/skills/adp-node-catalog starts_with /app/skills → OK
```

### Cambio en `worker/Dockerfile`

Agregar al final de la stage runtime (después del `COPY --from=builder`):

```dockerfile
COPY worker/skills /app/skills
```

El contexto de build es `apps/service/ia/platform/` (ver `cloudbuild.yaml`), así que la fuente del COPY es `worker/skills`, no `skills`.

### Cambio en `deploy_gcp.sh`

En la lista de variables del loop dentro de `build_env_vars()`, agregar `COLMENA_SKILLS_ALLOWED_DIRS` (con default fijo `/app/skills` en la sección "Runtime defaults").

```bash
# Runtime defaults
COLMENA_SKILLS_ALLOWED_DIRS=${COLMENA_SKILLS_ALLOWED_DIRS:-"/app/skills"}

# en build_env_vars(), agregar al for loop:
for var in OPENAI_API_KEY ANTHROPIC_API_KEY GEMINI_API_KEY \
           AMADEUS_CLIENT_ID AMADEUS_CLIENT_SECRET \
           COLMENA_POOL_MAX_ENTRIES COLMENA_POOL_MAX_CONN_PER_URL \
           COLMENA_POOL_MIN_CONN_PER_URL COLMENA_POOL_IDLE_TIMEOUT_SEC \
           COLMENA_POOL_MAX_LIFETIME_SEC COLMENA_POOL_ACQUIRE_TIMEOUT_SEC \
           COLMENA_SKILLS_ALLOWED_DIRS; do
```

Mantener el default no vacío (`/app/skills`) — así siempre se inyecta al worker, incluso si el operador no lo exporta en su `.env`.

## Orden de ejecución

1. **Pre-check Colmena local**:
   - `cargo check --workspace` desde `colmena/` para confirmar que los cambios pendientes compilan.
   - Confirmar que el `[patch]` local de colmena en `adp/.../platform/Cargo.toml` está comentado (el deploy lo valida y aborta si no, pero verificamos manual).
2. **Commit y push de Colmena a `develop`**:
   - Stagear modificaciones relevantes (D4) + `gcs_store.rs` untracked.
   - NO stagear `tests/graphs/external/skills/`, `tests/graphs/external/socketio_canvas_builder.json`, ni los grafos JSON modificados que sean tests personales.
   - Commit con mensaje descriptivo (GCS storage + python sandboxing + DocumentRuntime async + llm.rs ajustes).
   - `git push origin develop`.
   - Verificar con `git ls-remote https://github.com/Startti/colmena.git refs/heads/develop` que el hash remoto matchea HEAD local.
3. **Mover skills a ADP**:
   - `mkdir -p adp/apps/service/ia/platform/worker/skills/`
   - `mv colmena/tests/graphs/external/skills/adp-node-catalog adp/apps/service/ia/platform/worker/skills/`
   - Verificar layout: `ls adp/apps/service/ia/platform/worker/skills/adp-node-catalog/` debe mostrar `SKILL.md` y `references/`.
4. **Editar `worker/Dockerfile`**: agregar `COPY worker/skills /app/skills`.
5. **Editar `deploy_gcp.sh`**: agregar `COLMENA_SKILLS_ALLOWED_DIRS` con default `/app/skills` y al loop de `build_env_vars()`.
6. **Borrar dir vacío en Colmena**: `rm -rf colmena/tests/graphs/external/skills/` (debería estar vacío después del `mv`, pero `rm -rf` por las dudas).
7. **Actualizar grafo de test local en Colmena** (opcional, solo si se quiere seguir testeando local): cambiar `socketio_canvas_builder.json` para que `paths` apunte al path absoluto de ADP en el home del usuario. Este archivo permanece untracked.
8. **Commitear cambios de ADP** (recomendado pero no estrictamente necesario — `gcloud builds submit` sube el contexto local):
   - Stagear: `worker/Dockerfile`, `deploy_gcp.sh`, `worker/skills/` completo, y los demás cambios pendientes en ADP que sean parte de este deploy (`cloudbuild.yaml`, `worker/Cargo.toml`, `worker/src/main.rs`, `Cargo.toml`).
   - Commit + push (push opcional; deploy no lo necesita).
9. **Ejecutar deploy**:
   - `cd adp/apps/service/ia/platform && source .env && ./deploy_gcp.sh`
   - Confirmar en el output que `Colmena commit:` muestra el hash que acabamos de pushear.
10. **Verificación post-deploy**:
    - `curl $WORKER_URL/` → 200 OK (health check).
    - Lanzar un grafo de prueba que use la skill (vía API) y confirmar en logs del worker:
      - No aparece error `loading filesystem skills:`.
      - Se emite evento `skill_loaded` con `source: "filesystem"` y `skill_name: "adp-node-catalog"`.
    - Probar al menos una referencia (`load_skill('adp-node-catalog', 'agent')`) y confirmar contenido devuelto.

## Riesgos y mitigaciones

| Riesgo | Mitigación |
|--------|------------|
| Deploy se ejecuta sin haber pusheado Colmena → Cloud Build resuelve develop viejo | El script imprime `Colmena commit:` antes de buildear; verificar manualmente que el hash matchea el último commit local antes de aceptar el build. |
| `[patch]` local activo en `apps/service/ia/platform/Cargo.toml` | El script aborta con error explícito (línea 82-86 de `deploy_gcp.sh`). |
| `Cargo.lock` borrado en `git status` | Es intencional: el script lo elimina (`rm -f Cargo.lock`) para forzar resolución fresca de develop. |
| Cambios sin commitear en ADP en archivos del deploy (`cloudbuild.yaml`, etc.) | `gcloud builds submit .` sube el contexto local, así que esos cambios SÍ entran al build. Riesgo mitigado de hecho — pero conviene commitear para trazabilidad. |
| Skill referenciada por absoluto `/app/skills/...` no funciona local | Por diseño. Para testing local hay un grafo separado (untracked) con path al home del usuario. |
| Skill nueva tiene un `SKILL.md` malformado y el grafo no arranca | La validación es al **cargar el grafo**, antes de ejecutar nodos. El error sale en logs del worker como `loading filesystem skills: ...` y el job falla con claridad. |
| `COLMENA_SKILLS_ALLOWED_DIRS` no llega al worker (typo, separator) | Si falta, `graph_dir = /app` igual cubre `/app/skills/...`, así que el path validate pasa por accidente. Mitigación: smoke test post-deploy verifica explícitamente el evento `skill_loaded`. |

## Out of scope

- Versionado de skills (no hay `version:` en frontmatter; lo que se despliega es lo que está en el commit del worker).
- Hot-reload de skills (requiere redeploy del worker).
- Skills compartidas entre worker y otros servicios (decision D1: solo worker por ahora).
- Migración de los grafos de producción (ya generados por el canvas de ADP) para que usen `/app/skills/...` — fuera de alcance, asume que la UI ya genera los paths correctos o que se actualiza por separado.
- Refactor del LLM node para soportar interpolación de env vars en `skills.paths` (`${COLMENA_SKILLS_ROOT}/...`) — sería más portable, pero es trabajo de Colmena, no de este deploy.

## Criterios de éxito

1. `curl $WORKER_URL/` devuelve 200 después del deploy.
2. El logo del worker en Cloud Run muestra `Colmena commit:` igual al último commit pusheado a `develop` antes del deploy.
3. Un grafo que usa `"paths": ["/app/skills/adp-node-catalog"]` se ejecuta end-to-end sin errores de carga de skills.
4. Los logs del worker emiten al menos un evento `skill_loaded` con `source: "filesystem"`, `skill_name: "adp-node-catalog"`.
5. El directorio `colmena/tests/graphs/external/skills/` ya no existe en el repo de Colmena después de la operación.
