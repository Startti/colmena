# 24. Skills

Skills son paquetes de conocimiento en markdown que los nodos LLM cargan **bajo demanda** durante la ejecución. La idea es la misma que en Claude Code y Gemini CLI: el modelo ve un catálogo con nombre + descripción de cada skill disponible, y decide activar una skill concreta llamando al tool sintético `load_skill`. Esto evita inflar el system prompt con conocimiento que quizá no se use.

## Estructura de una skill

```
python-expert/                  # el nombre del directorio ES el nombre canónico
  SKILL.md                      # obligatorio
  references/                   # opcional
    frameworks.md
    testing.md
```

### Frontmatter de `SKILL.md`

```markdown
---
name: python-expert
description: Use when the user asks about Python typing, async, or stdlib. Not for general programming questions.
references:
  - name: frameworks
    description: Detailed notes on Django, FastAPI, and Flask
---

# Python Expert

You are an expert in modern Python (3.11+)...
```

- `name` (obligatorio): debe coincidir exactamente con el nombre del directorio.
- `description` (obligatorio): lo que el LLM ve en el catálogo. Incluir *cuándo usar* y *cuándo NO usar*.
- `references` (opcional): array de `{name, description}`. Cada `<name>` debe corresponder a un archivo `references/<name>.md` en disco.

## Skills integradas en el código (built-in)

Skills compiladas en el crate vía `include_dir!`. Viven en `src/libs/colmena/skills/`. Se distribuyen con la librería — cualquier usuario de Colmena puede activarlas por nombre:

```json
{
  "type": "llm_call",
  "config": {
    "skills": {
      "builtin": ["python-expert", "sql-optimizer"]
    }
  }
}
```

Para agregar una nueva built-in skill: crea el directorio bajo `src/libs/colmena/skills/`, confirma que compila (`cargo build`), y comitea.

## Skills del usuario (paths)

### Caso 1 — una sola skill

Si el path apunta a un directorio que **contiene `SKILL.md` directamente** en su raíz, se interpreta como una skill individual:

```
my-skill/
  SKILL.md
  references/
    notes.md
```

```jsonc
"skills": { "paths": ["./my-skill"] }
```

### Caso 2 — una carpeta con muchas skills (root mode)

Si el path apunta a un directorio que **NO contiene `SKILL.md` en su raíz**, se interpreta como un **root de skills** y escanea cada subdirectorio inmediato que tenga `SKILL.md`. Hijos sin `SKILL.md` (archivos sueltos, carpetas sin skill, etc.) se ignoran silenciosamente:

```
my-skills/                       ← le pasás ESTE path
├── sales-analysis/              ← skill 1 (con references)
│   ├── SKILL.md
│   └── references/
│       ├── kpis.md
│       └── tables.md
├── expense-analysis/            ← skill 2 (con references)
│   ├── SKILL.md
│   └── references/
│       └── categories.md
├── customer-context/            ← skill 3 (sin references — válido)
│   └── SKILL.md
└── notes.txt                    ← ignorado silenciosamente
```

```jsonc
"skills": { "paths": ["./my-skills"] }
```

Las 3 skills se cargan automáticamente, con sus referencias listas para `load_skill`.

### Reglas de auto-detección

| Si el path... | El motor lo interpreta como | Carga |
|---|---|---|
| Tiene `SKILL.md` directo en su raíz | Skill individual | Solo esa skill |
| NO tiene `SKILL.md` en su raíz | Root de skills | Cada subdirectorio inmediato que tenga `SKILL.md` (un solo nivel — no busca recursivamente más profundo) |

**Importante**: el escaneo es de **un solo nivel**. Si tenés `my-skills/finance/sales-analysis/SKILL.md`, no se descubre — apuntá `paths` a `./my-skills/finance` (un entry por subgrupo) o aplaná la estructura.

### Resolución de rutas

- **Relativas** (`./my-skills`, `../shared`) → se resuelven contra el **directorio del JSON del grafo**, no contra el CWD del proceso.
- **Absolutas** (`/opt/colmena/team-skills`) → usadas tal cual.

### Symlinks

Cada path se valida con `canonicalize()`. Symlinks que apuntan **dentro** de los directorios permitidos se siguen. Symlinks que escapan al exterior se **skipean silenciosamente** (no crashea) durante el escaneo de root; si el path raíz mismo escapa, se rechaza con `PathNotAllowed`.

### Combinando con built-in

Se pueden mezclar en el mismo `llm_call`:

```jsonc
"skills": {
  "builtin": ["python-expert"],                        // del crate compilado
  "paths":   ["./domain-skills", "/opt/shared-skills"] // del filesystem
}
```

Todas terminan en el mismo `SkillRepository` (vía `CompositeSkillRepository`). El catálogo de `load_skill` es la unión menos las layer-1 guides (excluidas) y menos las layer-2 no-descubiertas (gated por `describe_tool`).

### Errores duros al cargar el grafo

El motor valida estas reglas al construir el repo. Cualquier violación aborta la carga del grafo:

| Error | Causa | Mensaje típico |
|---|---|---|
| `PathNotAllowed` | Path está fuera del directorio del grafo y no está en `COLMENA_SKILLS_ALLOWED_DIRS` | `"path '<canonical>' is not inside any allowed directory"` |
| `NotADirectory` | El path resuelve a un archivo, no a un directorio | `"path '<canonical>' is not a directory"` |
| `EmptyRoot` | Root sin ningún subdirectorio que tenga `SKILL.md` | `"root '<canonical>' contains no skill directories"` |
| `NameMismatch` | El campo `name:` del frontmatter no coincide con el nombre del directorio | `"skill name 'foo' does not match directory name 'bar' in <path>"` |
| `ReferenceFileMissing` | El frontmatter declara `references: [{name: kpis}]` pero falta `references/kpis.md` | `"reference file missing for skill '<name>': expected <path>"` |
| `SkillNameCollision` | Dos skills (de paths distintos o built-in vs path) tienen el mismo `name:` | `"skill '<name>' is declared in multiple locations"` |
| `FileTooLarge` | `SKILL.md` o una reference supera 64 KB | `"file too large: <path> (<size> bytes, limit 65536)"` |
| `InvalidFrontmatter` | YAML inválido o falta `name`/`description` | `"invalid frontmatter in <path>: <reason>"` |

Las validaciones ocurren **al cargar el grafo** (antes de ejecutar cualquier nodo) — si una skill está mal escrita, el grafo ni siquiera arranca. Eso evita errores en medio de una conversación con el LLM.

### Cómo se usan las references

Una skill puede dividir su contenido entre un `SKILL.md` corto (overview que sale a la primera) y varios `references/*.md` (cada uno hasta 64 KB, on demand).

```markdown
---
name: sales-analysis
description: How to analyze sales data — KPIs, tables, pitfalls.
references:
  - name: kpis
    description: Detailed KPI formulas (revenue, AOV, conversion).
  - name: tables
    description: Schema of orders, order_items, customers tables.
---

# Sales analysis — overview

Sales data live in `public.orders`. Common KPIs: revenue, AOV, top SKUs.
For KPI formulas load reference `kpis`; for table schemas load `tables`.
```

En runtime:
- `load_skill("sales-analysis")` → devuelve el body del `SKILL.md` (overview).
- `load_skill("sales-analysis", "kpis")` → devuelve `references/kpis.md` (detalle).
- `load_skill("sales-analysis", "tables")` → devuelve `references/tables.md`.

El LLM decide qué cargar en función de la sub-tarea. Las references **no aparecen como tools separadas** — son sub-recursos que la propia tool `load_skill` puede pedir.

## Seguridad: allowed directories

Por defecto, Colmena solo acepta paths *dentro* del directorio del JSON del grafo. Para permitir directorios compartidos, configura la variable de entorno:

```bash
COLMENA_SKILLS_ALLOWED_DIRS=/home/user/skills:/opt/colmena/shared-skills
```

Paths se validan con `canonicalize()`, lo que impide escapes vía `../` o symlinks hacia afuera. Las validaciones ocurren al **cargar el grafo** — si una skill está mal, el grafo ni siquiera arranca.

## Límites

| Límite | Valor |
|--------|-------|
| Tamaño de `SKILL.md` | 64 KB |
| Tamaño de cada reference | 64 KB |
| Skills activas por nodo | 50 |
| Extensión permitida | `.md` |

## Observabilidad

Cuando el LLM invoca `load_skill`, el stream SSE emite **tres** eventos:

1. `tool_call` / `LlmToolCallStart` — estándar de tool calling.
2. `skill_loaded` — evento enriquecido con `{skill_name, reference, source, size_bytes}`.
3. `tool_result` / `LlmToolCallFinish` — estándar.

El summary final del nodo incluye un campo `skills_used` cuando al menos una skill fue cargada:

```json
{
  "skills_used": [
    {
      "name": "python-expert",
      "source": "builtin",
      "references_loaded": ["frameworks"],
      "load_count": 1
    }
  ]
}
```

## Trust model

Activar una skill es equivalente a inyectar un system prompt escrito por otra persona. Colmena valida que la skill sea sintácticamente correcta y que viva en un directorio permitido, pero **no** valida el contenido semántico. Una skill hostil puede incluir instrucciones que engañen al LLM (prompt injection). Solo activa skills de autores en los que confías.

Qué sí controla Colmena:
- El LLM solo puede cargar skills del catálogo (enforced vía `enum` en el schema del tool).
- El contenido markdown nunca se ejecuta como código.
- El catálogo se fija al cargar el grafo — el LLM no puede añadir skills nuevas en runtime.

## Referencia rápida

- Tool expuesto al LLM: `load_skill(name: string, reference?: string)`.
- La descripción del tool contiene el catálogo completo (nombre + descripción de cada skill).
- Si no se configura `skills`, todo el sistema de skills queda desactivado (zero overhead).
## Layered routing

A skill's role is derived from how it's wired, not from where the file
lives. All skills share the same pool.

| Role | How it's marked | How it reaches the model |
|---|---|---|
| Layer 1 — node-type guide | frontmatter `node_type: <name>` | auto-folded into the tool context block of every tool with matching node_type; **never** in the `load_skill` catalog |
| Layer 2 — tool-scoped specific | referenced in `tool_configurations.<name>.skills` | appears in the `load_skill` catalog **only after** the parent tool is discovered (lazy `discovered_set`); in non-lazy mode, visible from turn 1 |
| Layer 3 — free-standing general | referenced in `llm_call.skills` and no `node_type` | always in the `load_skill` catalog (today's behavior) |

Validations at graph load:
- At most one skill per node_type. Two guides claiming the same
  `node_type` → hard error.
- A `tool.skills` reference to an unknown name → hard error.
- A `tool.skills` reference to a skill marked as a node_type guide →
  hard error.
- A `llm_call.skills` reference to a node_type guide → warning, ignored.

## How skills auto-load

The engine derives the complete skill load list automatically — operators
only need to declare scoped skills in `tool_configurations.<name>.skills`.
No redundant enumeration in `llm_call.skills.builtin` is required.

**Built-in skills** (compiled into the crate via `include_dir!`):

- Auto-load when listed explicitly in `llm_call.skills.builtin`.
- Auto-load when referenced in any `tool_configurations.<name>.skills`
  (layer-2 auto-registration, via `augment_builtin_names`).
- Auto-load when their frontmatter declares `node_type: X` and any
  configured tool has `node_type: X` (layer-1 guide, zero config
  required).

**Path-based skills** (from `llm_call.skills.paths`): every `SKILL.md`
found under those directories is discovered automatically — no explicit
enumeration needed.

**Operator's only declarative job:** list scoped layer-2 skill names in
`tool_configurations.<name>.skills`. Everything else is derived.

### Before (old, required duplication)

```json
{
  "type": "llm_call",
  "config": {
    "skills": {
      "builtin": ["sales-analysis"]
    },
    "tool_configurations": {
      "query_sales": {
        "node_type": "sql_query",
        "skills": ["sales-analysis"]
      }
    }
  }
}
```

### After (current, single declaration)

```json
{
  "type": "llm_call",
  "config": {
    "tool_configurations": {
      "query_sales": {
        "node_type": "sql_query",
        "skills": ["sales-analysis"]
      }
    }
  }
}
```

The engine auto-derives that `sales-analysis` must be loaded (because it
is referenced in `tool.skills`) and that `sql_query`'s node-type guide
must be auto-folded (because `query_sales` has `node_type: sql_query`).

- Diseño completo: [docs/superpowers/specs/2026-04-20-llm-skills-design.md](../superpowers/specs/2026-04-20-llm-skills-design.md)
