# Built-in skills

Each subdirectory here is compiled into the `colmena_dag_engine` crate at build time via the `include_dir!` macro and becomes available to LLM nodes as a built-in skill.

## How to add a skill

1. Create a directory named after the skill (e.g. `python-expert/`). The directory name is the skill's canonical name.
2. Add a `SKILL.md` file with YAML frontmatter (`name`, `description`, optional `references`). `name` must match the directory name.
3. For each entry in `references`, add `references/<name>.md`.
4. Keep each file under 64 KB.

See `docs/developer_guide/24_skills.md` for the full contract.
