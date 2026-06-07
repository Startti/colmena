# colmena LLM-facing text

This folder holds every Rust-native string the LLM reads. Edit a file here
to change what the model sees — you do not need to touch source code.

## Layout

- `prompts/` — monolithic system messages and preludes. One file per text.
  - `python_sandbox/` — the auto-prelude/postlude blocks wrapped around
    user code inside `crdt_doc_run_python` and `gsheets_run_python`.
- `tools/` — YAML registries, one file per toolkit package. Each top-level
  key is a tool's registered `name` constant. Two sub-keys: `summary` (≤
  200 chars one-line) and `description` (multi-line).

## How to add a new tool's text

1. Open `tools/<package>.yaml`.
2. Append an entry:

   ```yaml
   <tool_name>:
     summary: A short one-liner shown in the lazy-loading catalog.
     description: |
       Full description visible to the LLM when the tool is called.
   ```

3. Run `cargo test --lib text` — the loader and `every_registered_tool_has_text_entry`
   test verify your YAML parses and matches a registered builder.

## How to add a new prompt

1. Create a new `.md` file under `prompts/`.
2. In the Rust caller, swap the inline string for
   `include_str!("../../<...>/text/prompts/<name>.md")`. Count `..` to
   reach `src/libs/colmena/` from the calling file.

## Why this layout exists

See `docs/superpowers/specs/2026-06-06-text-centralization-design.md`.
