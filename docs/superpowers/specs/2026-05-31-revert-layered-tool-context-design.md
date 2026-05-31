# Revert layered-tool-context + recursive references + skills_path

**Fecha:** 2026-05-31
**Autor:** Daniel Garcia (con asistencia de Claude)
**Estado:** Draft — pending user review
**Reverts:** [2026-05-29-layered-tool-context-design.md](2026-05-29-layered-tool-context-design.md) en gran parte.

---

## 1. Resumen ejecutivo

El feature `layered-tool-context` introdujo 3 capas para inyectar información a los LLMs sobre las tools:

1. **Layer 1a — node-type guide auto-fold:** skills con `node_type: X` frontmatter se auto-attacheaban a la descripción de cualquier tool del node type X.
2. **Layer 1b — tool_description_supplement:** hook de Rust que computa texto desde el config del nodo (ej. SQL permissions) y lo auto-attachea.
3. **Layer 2 — tool-scoped skills:** `tool_configuration.skills: [...]` campo por tool, gated por `discovered_set`.
4. **Layer 3 — free-standing skills:** `llm_call.skills: [...]` con catálogo de `load_skill`. (Pre-existía.)

**Este spec revierte layers 1a + 1b + 2** y todo el plumbing asociado. Mantiene solo **layer 3** (el mecanismo donde el LLM elige qué skill cargar). Agrega dos features encima:

- **References recursivas** — un archivo de `references/*.md` puede tener su propio frontmatter con `references: [...]`. El LLM navega un árbol via `load_reference(skill_name, path)`.
- **`skills_path` en `llm_call`** — además de `skills: [<names>]` (existente), el nodo `llm_call` acepta `skills_path: <dir>` o `skills_paths: [<dirs>]` que cargan todas las skills bajo ese directorio sin tener que enumerarlas.

---

## 2. Motivación

El usuario consideró el sistema layered **overengineering**:

- 3 capas con reglas distintas de visibilidad (auto-fold vs gated discovery vs always visible) es difícil de razonar.
- El acoplamiento skills↔nodos via `node_type` frontmatter es magic-by-convention. Renombrar un node type rompe la conexión silenciosamente.
- `tool_description_supplement` inyecta texto sin que el LLM lo pida — viola el principio "el LLM elige qué cargar".
- Un solo mecanismo (skills referenciadas explícitamente, con referencias recursivas) cubre todos los casos:
  - "Best practices del tool X" = skill autorada por el usuario, referenciable desde `llm_call.skills`.
  - "Política de permisos SQL" = skill autorada describiendo la política. El validador runtime sigue enforced las reglas; la info al LLM es opt-in.
  - "Doc específico al subnodo Y" = reference dentro de otra skill (recursivo).

---

## 3. Goals & Non-goals

### Goals

- Borrar todo el código y data structures de layers 1a, 1b, 2.
- Mantener la API de `llm_call.skills` + `load_skill` + `load_reference` con el mismo schema externo (compatibilidad con grafos existentes que solo usaban layer 3).
- Extender `load_reference` para aceptar paths con `/` que naveguen sub-referencias.
- Permitir frontmatter `references: [...]` en cualquier `.md` dentro de `references/` (no solo en `SKILL.md`).
- Validar ciclos en references recursivas (rechazar `A → B → A`).
- Agregar `skills_path` y `skills_paths` al config de `llm_call`.

### Non-goals

- No tocamos el formato base de skills (sigue siendo `SKILL.md` + frontmatter YAML + opcional `references/`).
- No cambiamos los validadores runtime de los nodos (ej. SqlPermissions sigue rechazando queries que violan política — solo el LLM ya no recibe el texto que se las cuenta upfront).
- No tocamos `load_skill` o `load_attachment` semánticamente, solo limpieza de plumbing layered.
- No migramos skills existentes con `node_type` automáticamente — el built-in `sql_query-guide` se borra. Si el usuario quiere esa info, autorea una skill propia.

---

## 4. Cambios concretos

### 4.1. Código a borrar

| Path | Qué |
|---|---|
| `src/libs/colmena/src/skills/domain/skill_catalog.rs` (o donde viva) | Campo `node_type` de `SkillCatalogEntry`; método `find_by_node_type` |
| `src/libs/colmena/src/skills/infrastructure/builtin_skill_repository.rs` | Indexing por `node_type`; validación duplicate-guide para node_type |
| `src/libs/colmena/src/skills/infrastructure/composite_skill_repository.rs` | Idem |
| `src/libs/colmena/src/skills/...` (parser de frontmatter) | Parsing de campo `node_type:` |
| `src/libs/colmena/src/dag_engine/domain/tool_configuration.rs` | Campo `skills: Vec<String>` |
| `src/libs/colmena/src/dag_engine/domain/node.rs` | Trait method `tool_description_supplement` |
| `src/libs/colmena/src/dag_engine/domain/sql_permissions.rs` | Método `describe_policy_for_llm` |
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/sql.rs` | Impl `tool_description_supplement` para SqlNode |
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/tool_context.rs` | **Borrar archivo entero** |
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/describe_tool.rs` | Toda la lógica de tool context block; vuelve a render solo description + parameters table |
| `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs` | Append de tool context block a `ToolDefinition.description`; el unify de skill_repo simplificado |
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs` | Per-request catalog rebuild basado en discovered_set + tool.skills; vuelve a catálogo estático basado SOLO en `llm_call.skills`. Quitar `tool_context_blocks` de `extra_info` |
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/load_skill_tool.rs` | `filter_visible_skills` y `build_load_skill_tool_definition_with_catalog` se simplifican (catalog ya no se filtra por discovered_set) |
| `src/libs/colmena/skills/sql_query-guide/` | Borrar skill built-in (tiene `node_type: sql_query`) |
| `tests/graphs/agents/sql_layered_tool_context.json` | Borrar grafo e2e |
| `tests/graphs/agents/inventory_roleplay_*.json` (los que usan layered) | Borrar o convertir a layer 3 puro |
| `docs/superpowers/specs/2026-05-29-layered-tool-context-design.md` | Mantener pero marcar como **superseded by este spec** |
| `docs/superpowers/plans/2026-05-29-layered-tool-context.md` | Mantener pero marcar superseded |

### 4.2. Código a modificar

| Path | Cambio |
|---|---|
| `docs/developer_guide/24_skills.md` | Reescribir secciones sobre layered (eliminar layer 1/2 docs); agregar sección sobre **recursive references** + **skills_path** |
| Parser de frontmatter | Permitir `references: [...]` también en archivos dentro de `references/` (hoy solo en `SKILL.md`) |
| `SkillRepository::load_reference` | Aceptar segundo arg como path con `/` (ej. `"frameworks/django"`). Resolver recursivamente |
| `load_skill_tool.rs` o `load_reference` synthetic tool | Schema acepta path. Documentar en description del tool |
| Validador de skills al cargar grafo | Detectar ciclos en references recursivas → hard error |
| `llm_call` config | Aceptar `skills_path: String` y `skills_paths: Vec<String>` además de `skills: Vec<String>`. Resolver paths a lista de skill names (union, deduped) |

### 4.3. Código a conservar

- `Skill` y `SkillCatalogEntry` (sin `node_type`)
- `SkillRepository` core (`load_from_dir`, `find_by_name`, etc.)
- `BuiltinSkillRepository`, `CompositeSkillRepository`, `PathSkillRepository`
- `llm_call.skills: [<names>]` field
- `load_skill(name)` synthetic tool
- Skills built-in que NO usan `node_type` (ej. `sales-analysis`, `expense-analysis` si las querés mantener — sino borrar tmb)
- Validaciones existentes: nombres únicos, 64KB limit, 50 active skills por nodo

---

## 5. Cambios al modelo de datos

### 5.1. `Skill` / `Reference`

**Hoy:**
```rust
pub struct Skill {
    pub name: String,
    pub description: String,
    pub content: String,           // SKILL.md body
    pub references: Vec<ReferenceMeta>,
    pub node_type: Option<String>, // ← borrar
}

pub struct ReferenceMeta {
    pub name: String,
    pub description: String,
    // implicitly: file at references/<name>.md
}
```

**Después:**
```rust
pub struct Skill {
    pub name: String,
    pub description: String,
    pub content: String,
    pub references: Vec<ReferenceMeta>,
}

pub struct ReferenceMeta {
    pub name: String,
    pub description: String,
    pub references: Vec<ReferenceMeta>,  // ← NUEVO: nested references
}
```

Y el repository indexa references por path: `(skill_name, [ref1, ref2, ...])`.

### 5.2. `tool_configuration`

**Hoy:**
```rust
pub struct ToolConfiguration {
    // ... existing fields
    pub skills: Vec<String>,  // ← borrar
}
```

**Después:** sin campo `skills`.

### 5.3. `llm_call` config

**Hoy (schema-derived):**
```json
{
  "skills": ["python-expert", "sales-analysis"]
}
```

**Después:**
```json
{
  "skills": ["python-expert", "sales-analysis"],   // opcional (existente)
  "skills_path": "/path/to/my-skills",             // opcional (NUEVO)
  "skills_paths": ["/path1", "/path2"]             // opcional (NUEVO, alternativa a singular)
}
```

Resolution: union de `skills` (por nombre) + todas las skills bajo los paths. Sin duplicados (mismo nombre = misma skill).

### 5.4. Synthetic tool `load_reference`

**Hoy:**
```json
{
  "name": "load_reference",
  "parameters": {
    "skill_name": "python-expert",
    "reference_name": "frameworks"
  }
}
```

**Después:**
```json
{
  "name": "load_reference",
  "parameters": {
    "skill_name": "python-expert",
    "reference_name": "frameworks"           // OR "frameworks/django" OR "frameworks/django/templating"
  }
}
```

El schema externo no cambia (sigue siendo `reference_name: string`), pero la string ahora puede ser un path con `/`. Documentado en la description del tool.

---

## 6. Validaciones nuevas

| Condición | Acción |
|---|---|
| Reference frontmatter declara una sub-reference que no existe en disco | Hard error al cargar skill: `"reference 'X' references unknown sub-reference 'Y'"` |
| Ciclo en references: `A → B → A` | Hard error: `"reference cycle detected: A → B → A"` |
| Profundidad máxima de references (límite arbitrario para evitar bombs) | 5 niveles. Más profundo = hard error |
| `llm_call.skills_path` apunta a directorio inexistente | Hard error al cargar grafo |
| `llm_call.skills_path` apunta a directorio vacío (sin SKILL.md adentro) | Warning, lista resultante vacía |
| Skill con `node_type` frontmatter (legacy) | Hard error al cargar: `"skill 'X' uses deprecated 'node_type' frontmatter — see migration guide"` |

---

## 7. API que ve el LLM (después del cambio)

Idéntica a hoy excepto:

- **`load_skill(name)`** — mismo schema, mismo comportamiento.
- **`load_reference(skill_name, reference_name)`** — `reference_name` ahora puede ser un path con `/`. Documentado en description: *"To navigate nested references, separate names with '/'. Example: 'frameworks/django/orm'."*
- **NO existe** `describe_tool` con tool context block (vuelve a la versión simple original)
- **NO existe** `tool_context_blocks` en `extra_info`

---

## 8. Estrategia de implementación

**Commit-by-commit revert + 2 feature commits al final**, en una sola branch / PR.

### Fase A — Revert layered (orden inverso al merge)

Aplico `git revert -n` a cada uno de estos commits en orden inverso (más reciente primero), con `--no-commit`, juntando todo en una pila de cambios. Luego un solo commit "revert: layered-tool-context (all 3 layers + plumbing)".

```
31cda9c  fix(llm): auto-derive also includes layer-1 guides matching tool node_types
1677519  feat(llm): auto-derive skill load list from tool.skills
7678255  feat(llm): tool_context_blocks in extra_info summary
9d03325  feat(llm): graph-load validation for skill wiring
b94f910  feat(llm): per-request load_skill catalog with layer 1/2/3 rules
16392e9  fix(executor): unify skill_repo with existing skill_repository
2abdce1  feat(llm): pipe SkillRepository + registry into describe_tool dispatch
34342a9  feat(executor): append tool context block to ToolDefinition.description
7fc4dd2  feat(llm): build_tool_context_block — layered tool block builder
08e617a  feat(tool_configuration): add skills: Vec<String> field
0f0befc  feat(sql): SqlNode implements tool_description_supplement
de97917  feat(sql): SqlPermissions::describe_policy_for_llm
d6d9630  feat(node): add tool_description_supplement hook to ExecutableNode
d40e5bc  feat(skills): author sql_query-guide as the first layer-1 guide
b190042  feat(skills): find_by_node_type + duplicate-guide validation
918acfd  feat(skills): surface node_type through SkillCatalogEntry
48d69be  feat(skills): parse optional node_type frontmatter
```

Si algún revert tiene conflicto (porque commits posteriores no-layered tocaron los mismos archivos), se resuelve manual file-by-file y se documenta.

### Fase B — Limpiar tests, skills built-in, docs

Borrar `sql_query-guide/`, borrar grafos e2e específicos, marcar docs como superseded.

### Fase C — Feature: recursive references

1 commit: extender parser + load_reference + tests.

### Fase D — Feature: skills_path en llm_call

1 commit: schema + resolver + tests.

### Fase E — Update docs

1 commit: actualizar `24_skills.md` con el modelo nuevo.

### Fase F — Verify

- `cargo test --lib`
- `cargo clippy -- -D warnings`
- `cargo fmt --check`
- Hexagonal compliance script
- E2E roleplay graphs (los que NO sean del layered)

---

## 9. Migración para usuarios existentes

| Caso | Migración |
|---|---|
| Grafo usa solo `llm_call.skills: [<names>]` | Sin cambios, funciona idéntico |
| Grafo usa `tool_configuration.skills: [...]` | El campo desaparece. Mover esos skill names a `llm_call.skills` |
| Skill autorada por usuario tiene `node_type:` frontmatter | Quitarlo. La skill se vuelve referenciable normalmente desde `llm_call.skills` |
| Usuario depende de la info de SQL permissions auto-injectada | Autorar una skill markdown describiendo la política, referenciarla desde `llm_call.skills` |
| Skills built-in que el usuario consumía: `sql_query-guide` | Borrada. Si la quiere, autorearla como path-based skill |

Cambios breaking serán comunicados en CHANGELOG.

---

## 10. Tests

### Unit
- Parser de frontmatter: rechaza `node_type:` (es lo opuesto a antes — antes lo aceptaba)
- `SkillRepository`: no tiene `find_by_node_type`
- `ToolConfiguration`: no acepta campo `skills`
- `Skill::references`: cada ReferenceMeta puede tener sub-references
- `load_reference("skill", "ref/sub/deeper")`: navega correctamente
- Detección de ciclo: `A → B → A` falla
- Profundidad máxima 5: falla
- `llm_call` config: acepta `skills_path` y `skills_paths`, unión funciona

### Integration / E2E
- Grafo con `llm_call.skills: [...]` (sin paths): funciona como hoy
- Grafo con `skills_path: "tests/fixtures/my-skills"`: catálogo incluye todas
- Grafo con skill que tiene recursive references: LLM puede navegar 3 niveles
- Grafo legacy con `tool_configuration.skills: [...]`: falla con error claro al cargar
- Grafo legacy con skill `node_type: X`: falla con error claro al cargar

### Regression
- Todos los tests que NO sean del layered feature deben seguir pasando.

---

## 11. Riesgos

| Riesgo | Mitigación |
|---|---|
| Reverts en cascada tienen conflictos por orden de commits | Revert en orden inverso; si conflict, resolver manual y documentar |
| Algún grafo en producción usa layered y se rompe al actualizar | Errores claros con instrucción de migración en el mensaje |
| Recursive references creando bombs de tokens | Profundidad máxima 5 + 64KB por archivo (existente) + 50 active skills (existente) |
| El revert toca también código del HTML PR (recién mergeado) | Los archivos de layered y HTML son disjoint en su mayoría. Verificar pre-flight con `git diff` |

---

## 12. Documentación a actualizar

- `docs/developer_guide/24_skills.md` — quitar layered, agregar nested refs + skills_path
- `docs/CHANGELOG_2026-05.md` — breaking change announcement
- `docs/superpowers/specs/2026-05-29-layered-tool-context-design.md` — header marcando "superseded by 2026-05-31-revert-layered-tool-context-design"
- `docs/superpowers/plans/2026-05-29-layered-tool-context.md` — header marcando superseded
- `docs/CODEBASE_TOUR.md` — actualizar sección de skills/tool context si la menciona

---

## 13. Plan next steps

1. Spec approved by user
2. Invoke writing-plans skill → plan paso-a-paso con TDD
3. Ejecutar plan (1 PR, varias commits agrupados por fase A-F)
4. PR review → merge a develop
