---
name: documentation
description: Protocol for maintaining repository documentation. Use when modifying code that impacts existing documentation or requires new docs.
---

# Documentation Skill

Detailed instructions for maintaining and updating documentation in the `docs/` folder of this repository.

## When to use this skill

- Use this whenever code changes have been made and need to be documented.
- Use this to ensure the `docs/` folder reflects the current state of the codebase.
- Use this when performing technical debt cleanup related to documentation.

## How to use it

### 1. Verify Changes with Git
- Before updating documentation, **always** run `git diff` (or use internal tools to see changes) to understand exactly what has been modified in the code.
- Analyze the diff to identify new functions, modified logic, structural changes, or updated dependencies.

### 2. Update Relevant Documentation
- Identify which files in the `docs/` directory or in-code documentation (e.g., Python docstrings, Rustdoc comments `///`) are affected by the code changes.
- Update the documentation to reflect the new implementation details, architecture, or usage patterns.
- If a new feature or module is added:
    - Create a new documentation file in `docs/` following the existing naming conventions.
    - Ensure public Rust APIs are documented using triple-slash (`///`) comments.
    - Ensure Python functions have Google-style or standard docstrings.

### 3. Maintain Consistency
- Ensure that descriptions, diagrams, and examples in the documentation are consistent with the latest code.
- For Rust code, run `cargo doc --no-deps --open` locally to verify that the generated documentation looks correct.
- Check for technical debt in documentation (e.g., outdated screenshots, broken links, or misleading descriptions) and fix them alongside code changes.

### 4. Verification
- Confirm that the updated documentation accurately describes the "what" and the "how" of the changes.
- Ensure the `docs/` folder is well-organized and follows repository standards.
