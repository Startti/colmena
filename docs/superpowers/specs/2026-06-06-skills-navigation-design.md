# Skills navigation + built-in skills index — design

**Status**: Approved (2026-06-06)
**Author**: Daniel García + colmena agent
**Tracks**: E-T19 (skills index doc + README upgrade + CI test)
**Related**:
[2026-06-06 text centralization spec](2026-06-06-text-centralization-design.md),
[`docs/developer_guide/41_builtin_tools_index.md`](../../developer_guide/41_builtin_tools_index.md)
(the pattern this mirrors).

---

## 1. Goals

Make colmena's 8 built-in skills as discoverable and self-documenting as the
synthetic tools shipped in E-T18. Today the skills folder at
`src/libs/colmena/skills/` contains the canonical content but offers no
top-level navigation: a developer has to open each `SKILL.md` to see what
that skill does. We ship three artifacts:

1. **`docs/developer_guide/42_builtin_skills_index.md`** — user-facing
   reference, one row per skill: name + one-line description + link to the
   skill's `SKILL.md`.
2. **`src/libs/colmena/skills/README.md`** — upgraded contributor-facing
   navigation guide: tabular listing + how to add a new skill.
3. **CI test `index_doc_covers_all_registered_skills`** — iterates
   `skills/` at test time and asserts every folder containing a `SKILL.md`
   has an entry in the user-facing index. Closes the drift loop.

## 2. Non-goals

- Rearranging the existing skill folders or renaming any skill.
- Changing the `load_skill` runtime behavior.
- Auto-generating the index doc. Hand-maintained with a CI completeness
  test, mirroring E-T18a.
- Promoting any skill to a higher-status doc (e.g. its own dev-guide
  section). Skills stay in `skills/`.

## 3. Open-source rule

The user-facing index lists only colmena-shipped skills, all generic and
domain-neutral. No ADP-specific skill names or content. If ADP wants to
ship its own skills, they live in a separate folder loaded via the same
`load_skill` mechanism — out of scope here.

## 4. Components

### 4.1 The 8 (or 9, counting `_placeholder`) skills today

Inventory by reading `src/libs/colmena/skills/`:

- `_placeholder` (kept for the `include_dir!` empty-folder contract; not a real skill)
- `crdt-doc-cross-sheet-analysis`
- `crdt-doc-formulas`
- `crdt-doc-run-python`
- `expense-analysis`
- `gsheets-cross-sheet-analysis`
- `python-expert`
- `sales-analysis`
- `sql-optimizer`

Each real skill has a `SKILL.md` with YAML frontmatter declaring `name`,
`description`, and other metadata. The user-facing index pulls the
description from the frontmatter.

### 4.2 User-facing index (`docs/developer_guide/42_builtin_skills_index.md`)

Sections:

1. **Intro**: one paragraph explaining what a skill is and how the LLM
   discovers them (`load_skill` tool).
2. **The skills**: a single table — one row per skill.

   ```
   | Skill | What it teaches | SKILL.md |
   |---|---|---|
   | `gsheets-cross-sheet-analysis` | <first-line description from frontmatter> | [link to SKILL.md] |
   | …
   ```

3. **How to use a skill**: short snippet showing `load_skill` invocation.
4. **How to add a new skill**: 3-step recipe pointing at the folder
   convention.

### 4.3 Skills README upgrade (`src/libs/colmena/skills/README.md`)

The existing README is the contributor-side counterpart to the
user-facing doc. It gains:

- A "**Quick navigation**" table mirroring the user-facing doc.
- A "**How to add a new skill**" section with the exact directory layout
  and frontmatter shape.
- A "**Naming convention**" note: skill folder names are kebab-case;
  uniqueness is enforced by the `include_dir!` build (compile error on
  duplicate).

### 4.4 CI test `index_doc_covers_all_registered_skills`

Lives next to the synthetic-tools coverage tests in
`src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs`
(extending the existing `text_coverage_tests` module).

Algorithm:

1. Embed the doc via `include_str!` at compile time.
2. At test time, iterate `src/libs/colmena/skills/` using `std::fs::read_dir`.
3. For each subdirectory containing a `SKILL.md`, extract the skill name
   (folder name, e.g. `gsheets-cross-sheet-analysis`).
4. Skip `_placeholder` explicitly.
5. Assert each skill name appears as a backtick-wrapped token in the doc.
6. On failure, list the missing skills in the assertion message.

The choice between `std::fs::read_dir` and `include_dir!` for the test:
prefer `read_dir` so the test fails locally when a developer forgets to
update the doc — not just at compile time.

## 5. Decisions / edge cases

| Case | Decision |
|---|---|
| `_placeholder` directory | Exempt — it exists only for `include_dir!` empty-folder semantics |
| Skill folder without `SKILL.md` | Treated as not-a-skill (skipped by the test) |
| Skill folder with `SKILL.md` but no frontmatter `description` | Doc author must add a description manually; test only checks presence, not content quality |
| Two skills with similar names | Allowed; the index lists them as separate entries |
| ADP adds its own skill via downstream tooling | Not part of the colmena index; ADP maintains its own list |

## 6. Tests

- `index_doc_covers_all_registered_skills` (new) — described in §4.4.

That is the only new test. Failure mode is descriptive ("missing skill
entries: ['foo', 'bar']") so the fix is obvious.

## 7. Task breakdown

| ID | Title | Estimate |
|---|---|---:|
| **E-T19a** | Write `docs/developer_guide/42_builtin_skills_index.md` with one row per skill (8 entries) | 30 min |
| **E-T19b** | Upgrade `src/libs/colmena/skills/README.md` with the quick-navigation table and authoring recipe | 20 min |
| **E-T19c** | Add `index_doc_covers_all_registered_skills` test to `text_coverage_tests` module | 30 min |
| **E-T19d** | Docs sweep — `DEVELOPER_GUIDE.md` index entry, `CHANGELOG_2026-06.md`, optional `BACKLOG.md` entry for auto-gen v1.1 | 10 min |

Total: **~1.5 h** via subagent-driven.

## 8. Back-compat

| Existing usage | After change | Status |
|---|---|---|
| `load_skill("<name>")` runtime calls | Same names, same content | ✅ Unchanged |
| `skills/<name>/SKILL.md` paths | Untouched | ✅ Unchanged |
| Anything outside `docs/developer_guide/` or `skills/README.md` | Not modified | ✅ Untouched |

Zero break. No downstream consumer needs to change.

## 9. Future BACKLOG

- **Auto-generate `42_builtin_skills_index.md`** — read each
  `SKILL.md`'s frontmatter, emit the doc. The CI completeness test would
  become redundant.
- **Per-skill detailed dev-guide pages** — promote one or two key skills
  to first-class `docs/developer_guide/4X_<skill>.md` pages with examples
  and rationale, separately from the SKILL.md (which is what the LLM
  reads).
- **Skills tag / categorisation** — frontmatter gains `tags: [spreadsheet,
  data-analysis, …]` and the index groups by tag.

## 10. Self-review

- ✅ Placeholders: none.
- ✅ Internal consistency: §4 components match §7 tasks 1:1.
- ✅ Scope: single deliverable, no decomposition needed.
- ✅ Ambiguity: each row in the §5 table disambiguates a tricky case.
- ✅ Open-source rule: §3 explicit.
- ✅ Test coverage: §6 names the exact test and its assertion shape.
- ✅ Back-compat: §8 confirms zero break.
